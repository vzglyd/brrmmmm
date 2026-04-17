use std::io::{BufRead, Write};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};

mod abi;
mod controller;
mod events;
mod host;
mod persistence;

use controller::{SidecarController, inspect_wasm_contract, validate_inspection};
use events::{EnvVarStatus, Event, EventSink, now_ts};

#[derive(ValueEnum, Clone, Default, PartialEq)]
enum OutputFormat {
    #[default]
    Text,
    Json,
    Table,
}

#[derive(Parser)]
#[command(
    name = "brrmmmm",
    about = "Standalone sidecar runner for VZGLYD sidecar WASM modules",
    after_help = "\
EXAMPLES:
  brrmmmm validate sidecar.wasm
  brrmmmm validate sidecar.wasm --output table
  brrmmmm inspect  sidecar.wasm --output table
  brrmmmm run      sidecar.wasm --once
  brrmmmm run      sidecar.wasm --once --output json
  brrmmmm          sidecar.wasm              # launches TUI",
    version
)]
struct Cli {
    /// Output format: json, text, or table.
    /// Default: json for inspect, text for validate and run.
    #[arg(long, global = true, value_enum)]
    output: Option<OutputFormat>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run a sidecar WASM module
    Run {
        /// Path to the sidecar .wasm file
        wasm_path: String,

        /// Run a single fetch iteration and exit
        #[arg(long)]
        once: bool,

        /// Poll interval in seconds (default: 60)
        #[arg(long, default_value_t = 60)]
        interval: u64,

        /// Set environment variable (KEY=VALUE)
        #[arg(long, value_name = "KEY=VALUE")]
        env: Vec<String>,

        /// JSON object passed to the sidecar configure buffer
        #[arg(long, conflicts_with = "params_file")]
        params_json: Option<String>,

        /// Path to a JSON file passed to the sidecar configure buffer
        #[arg(long, value_name = "PATH")]
        params_file: Option<String>,

        /// Log channel pushes to stderr
        #[arg(long)]
        log_channel: bool,

        /// Emit structured NDJSON event stream to stdout (for TUI subprocess mode)
        #[arg(long)]
        events: bool,

        /// Verbose output
        #[arg(short, long)]
        verbose: bool,
    },

    /// Inspect a sidecar WASM module and print its contract
    Inspect {
        /// Path to the sidecar .wasm file
        wasm_path: String,
    },

    /// Validate that a sidecar WASM module loads correctly
    Validate {
        /// Path to the sidecar .wasm file
        wasm_path: String,
    },
}

fn find_tui_script() -> Option<std::path::PathBuf> {
    if let Ok(val) = std::env::var("BRRMMMM_TUI") {
        let p = std::path::PathBuf::from(val);
        if p.exists() {
            return Some(p);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            // Packaged install: tui/ lives next to the binary
            let p = dir.join("tui/dist/index.js");
            if p.exists() {
                return Some(p);
            }
            // Dev layout: target/release/brrmmmm → ../../tui/dist/index.js
            let p = dir.join("../../tui/dist/index.js");
            if p.exists() {
                return Some(p);
            }
        }
    }
    // CWD: user runs from within the cloned repo after cargo install --path .
    if let Ok(cwd) = std::env::current_dir() {
        let p = cwd.join("tui/dist/index.js");
        if p.exists() {
            return Some(p);
        }
    }
    None
}

fn launch_tui(args: &[String]) -> ! {
    let Some(tui) = find_tui_script() else {
        eprintln!(
            "[brrmmmm] TUI not found. Build it with: npm --prefix tui run build\n\
             [brrmmmm] Or set BRRMMMM_TUI=/path/to/tui/dist/index.js"
        );
        std::process::exit(1);
    };

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = std::process::Command::new("node")
            .arg(&tui)
            .args(args)
            .exec();
        eprintln!("[brrmmmm] failed to exec node: {err}");
        std::process::exit(1);
    }

    #[cfg(not(unix))]
    {
        let status = std::process::Command::new("node")
            .arg(&tui)
            .args(args)
            .status()
            .unwrap_or_else(|e| {
                eprintln!("[brrmmmm] failed to launch node: {e}");
                std::process::exit(1);
            });
        std::process::exit(status.code().unwrap_or(1));
    }
}

fn main() -> Result<()> {
    env_logger::init();

    let raw: Vec<String> = std::env::args().skip(1).collect();
    if let Some(first) = raw.first() {
        let known = ["run", "inspect", "validate"];
        if first.ends_with(".wasm") && !known.contains(&first.as_str()) {
            launch_tui(&raw);
        }
    }

    let cli = Cli::parse();

    match cli.command {
        Commands::Run {
            wasm_path,
            once,
            interval,
            env,
            params_json,
            params_file,
            log_channel,
            events,
            verbose,
        } => cmd_run(
            &wasm_path,
            once,
            interval,
            &env,
            params_json.as_deref(),
            params_file.as_deref(),
            log_channel,
            events,
            verbose,
            cli.output.unwrap_or(OutputFormat::Text),
        ),
        Commands::Inspect { wasm_path } =>
            cmd_inspect(&wasm_path, cli.output.unwrap_or(OutputFormat::Json)),
        Commands::Validate { wasm_path } =>
            cmd_validate(&wasm_path, cli.output.unwrap_or(OutputFormat::Text)),
    }
}

fn parse_env_vars(raw: &[String]) -> Vec<(String, String)> {
    raw.iter()
        .filter_map(|s| {
            s.split_once('=')
                .map(|(k, v)| (k.to_string(), v.to_string()))
        })
        .collect()
}

fn parse_params_bytes(
    params_json: Option<&str>,
    params_file: Option<&str>,
) -> Result<Option<Vec<u8>>> {
    let raw = if let Some(raw) = params_json {
        Some(raw.to_string())
    } else if let Some(path) = params_file {
        Some(std::fs::read_to_string(path).with_context(|| format!("read params file: {path}"))?)
    } else {
        None
    };
    let Some(raw) = raw else {
        return Ok(None);
    };

    let value: serde_json::Value =
        serde_json::from_str(&raw).context("sidecar params must be valid JSON")?;
    if !value.is_object() {
        anyhow::bail!("sidecar params must be a JSON object");
    }
    serde_json::to_vec(&value)
        .map(Some)
        .context("serialize sidecar params")
}

fn format_poll_strategy(poll: &abi::PollStrategy) -> String {
    match poll {
        abi::PollStrategy::FixedInterval { interval_secs } => {
            format!("fixed_interval {interval_secs}s")
        }
        abi::PollStrategy::ExponentialBackoff { base_secs, max_secs } => {
            format!("exponential_backoff base={base_secs}s max={max_secs}s")
        }
        abi::PollStrategy::Jittered { base_secs, jitter_secs } => {
            format!("jittered base={base_secs}s jitter={jitter_secs}s")
        }
    }
}

fn print_table(rows: &[(&str, String)]) {
    let key_w = rows.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
    let val_w = rows.iter().map(|(_, v)| v.len()).max().unwrap_or(0).min(60);
    let sep = "─".repeat(key_w + 2 + val_w);
    println!("{:<key_w$}  {}", "Field", "Value");
    println!("{sep}");
    for (k, v) in rows {
        println!("{:<key_w$}  {}", k, v);
    }
}

fn cmd_run(
    wasm_path: &str,
    once: bool,
    interval: u64,
    env: &[String],
    params_json: Option<&str>,
    params_file: Option<&str>,
    log_channel: bool,
    events_mode: bool,
    _verbose: bool,
    output: OutputFormat,
) -> Result<()> {
    let env_vars = parse_env_vars(env);
    let params_bytes = parse_params_bytes(params_json, params_file)?;

    let sink = if events_mode {
        EventSink::for_stdout()
    } else {
        EventSink::noop()
    };

    // Emit env snapshot so TUI knows which vars are present.
    if events_mode {
        sink.emit(Event::EnvSnapshot {
            ts: now_ts(),
            vars: EnvVarStatus::from_raw_env(&env_vars),
        });
    }

    if once {
        if !events_mode {
            eprintln!("[brrmmmm] running {wasm_path} in --once mode");
            eprintln!("[brrmmmm] starting sidecar, waiting for first channel_push...");
        }

        let controller = SidecarController::new(
            wasm_path,
            env_vars,
            params_bytes.clone(),
            log_channel,
            sink.clone(),
        )
        .with_context(|| format!("failed to load sidecar: {wasm_path}"))?;

        let timeout = std::time::Duration::from_secs(std::cmp::max(interval * 2, 30));
        let start = std::time::Instant::now();

        loop {
            if let Some(data) = controller.poll_output() {
                if !events_mode {
                    match output {
                        OutputFormat::Json => {
                            match serde_json::from_slice::<serde_json::Value>(&data) {
                                Ok(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap()),
                                Err(_) => {
                                    eprintln!("[brrmmmm] payload is not valid JSON, emitting raw");
                                    std::io::stdout().write_all(&data)?;
                                    std::io::stdout().write_all(b"\n")?;
                                }
                            }
                        }
                        OutputFormat::Table => {
                            match serde_json::from_slice::<serde_json::Value>(&data) {
                                Ok(serde_json::Value::Object(map)) => {
                                    let rows: Vec<(&str, String)> = map
                                        .iter()
                                        .map(|(k, v)| {
                                            let s = match v {
                                                serde_json::Value::String(s) => s.clone(),
                                                other => other.to_string(),
                                            };
                                            (k.as_str(), s)
                                        })
                                        .collect();
                                    print_table(&rows);
                                }
                                _ => {
                                    eprintln!("[brrmmmm] payload is not a JSON object, emitting raw");
                                    std::io::stdout().write_all(&data)?;
                                    std::io::stdout().write_all(b"\n")?;
                                }
                            }
                        }
                        OutputFormat::Text => {
                            std::io::stdout().write_all(&data)?;
                            std::io::stdout().write_all(b"\n")?;
                        }
                    }
                    eprintln!("[brrmmmm] payload received ({} bytes), stopping", data.len());
                }
                controller.stop();
                return Ok(());
            }

            if start.elapsed() > timeout {
                if !events_mode {
                    eprintln!(
                        "[brrmmmm] timeout waiting for channel_push ({}s)",
                        timeout.as_secs()
                    );
                }
                controller.stop();
                anyhow::bail!("timeout waiting for sidecar payload");
            }

            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    } else {
        if !events_mode {
            eprintln!("[brrmmmm] running {wasm_path} in daemon mode (interval: {interval}s)");
            eprintln!("[brrmmmm] sidecar controls polling; Ctrl+C to stop");
        }

        // Compute WASM hash for state persistence.
        let wasm_bytes =
            std::fs::read(wasm_path).with_context(|| format!("read WASM file: {wasm_path}"))?;
        let wasm_hash = persistence::wasm_identity(&wasm_bytes);
        drop(wasm_bytes);

        let controller =
            SidecarController::new(wasm_path, env_vars, params_bytes, log_channel, sink.clone())
                .with_context(|| format!("failed to load sidecar: {wasm_path}"))?;

        // In events mode, listen on stdin for TUI commands:
        //   force
        //   params_json {"key":"value"}
        if events_mode {
            let flag = controller.force_refresh_flag();
            let params_handle = controller.params_handle();
            let command_sink = sink.clone();
            std::thread::spawn(move || {
                let stdin = std::io::stdin();
                for line in stdin.lock().lines().map_while(Result::ok) {
                    let trimmed = line.trim();
                    if trimmed == "force" {
                        flag.store(true, std::sync::atomic::Ordering::Relaxed);
                    } else if let Some(raw) = trimmed.strip_prefix("params_json ") {
                        match serde_json::from_str::<serde_json::Value>(raw) {
                            Ok(value) if value.is_object() => match serde_json::to_vec(&value) {
                                Ok(bytes) => {
                                    *params_handle.lock().unwrap() = Some(bytes);
                                    flag.store(true, std::sync::atomic::Ordering::Relaxed);
                                    events::diag(
                                        &command_sink,
                                        "[brrmmmm] updated sidecar params and requested refresh",
                                    );
                                }
                                Err(error) => events::diag(
                                    &command_sink,
                                    &format!("[brrmmmm] failed to encode params_json: {error}"),
                                ),
                            },
                            Ok(_) => events::diag(
                                &command_sink,
                                "[brrmmmm] params_json command must contain a JSON object",
                            ),
                            Err(error) => events::diag(
                                &command_sink,
                                &format!("[brrmmmm] invalid params_json command: {error}"),
                            ),
                        }
                    }
                }
            });
        }

        let (tx, rx) = std::sync::mpsc::channel();
        ctrlc::set_handler(move || {
            let _ = tx.send(());
        })
        .context("set Ctrl+C handler")?;

        if !events_mode {
            eprintln!("[brrmmmm] press Ctrl+C to stop");
        }
        rx.recv().ok();

        if !events_mode {
            eprintln!("\n[brrmmmm] stopping...");
        }

        // Persist runtime state before stopping.
        let state = controller.snapshot();
        persistence::save(&wasm_hash, &state);

        controller.stop();
        Ok(())
    }
}

fn cmd_inspect(wasm_path: &str, output: OutputFormat) -> Result<()> {
    let inspection = inspect_wasm_contract(wasm_path)?;

    match output {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&inspection).unwrap());
        }
        OutputFormat::Text => {
            eprintln!("[brrmmmm] inspecting {wasm_path}");
            let d = inspection.describe.as_ref();
            println!(
                "logical_id:     {}",
                d.map(|d| d.logical_id.as_str()).unwrap_or("-")
            );
            println!(
                "name:           {}",
                d.map(|d| d.name.as_str()).unwrap_or("-")
            );
            println!("abi_version:    {}", inspection.abi_version);
            println!("size_bytes:     {}", inspection.wasm_size_bytes);
            println!(
                "entrypoint:     {}",
                inspection.entrypoint.as_deref().unwrap_or("-")
            );
            if let Some(d) = d {
                if let Some(poll) = &d.poll_strategy {
                    println!("poll_strategy:  {}", format_poll_strategy(poll));
                }
                println!("artifacts:      {}", d.artifact_types.join(", "));
                if !d.optional_env_vars.is_empty() {
                    let names: Vec<&str> = d
                        .optional_env_vars
                        .iter()
                        .map(|e| e.name.as_str())
                        .collect();
                    println!("optional_env:   {}", names.join(", "));
                }
            }
        }
        OutputFormat::Table => {
            let d = inspection.describe.as_ref();
            let mut rows: Vec<(&str, String)> = vec![
                (
                    "logical_id",
                    d.map(|d| d.logical_id.clone()).unwrap_or_default(),
                ),
                ("name", d.map(|d| d.name.clone()).unwrap_or_default()),
                ("abi_version", inspection.abi_version.to_string()),
                ("size_bytes", inspection.wasm_size_bytes.to_string()),
                (
                    "entrypoint",
                    inspection.entrypoint.clone().unwrap_or_default(),
                ),
            ];
            if let Some(d) = d {
                if let Some(poll) = &d.poll_strategy {
                    rows.push(("poll_strategy", format_poll_strategy(poll)));
                }
                rows.push(("artifacts", d.artifact_types.join(", ")));
                if !d.optional_env_vars.is_empty() {
                    let names: Vec<&str> = d
                        .optional_env_vars
                        .iter()
                        .map(|e| e.name.as_str())
                        .collect();
                    rows.push(("optional_env", names.join(", ")));
                }
            }
            print_table(&rows);
        }
    }
    Ok(())
}

fn cmd_validate(wasm_path: &str, output: OutputFormat) -> Result<()> {
    let inspection = inspect_wasm_contract(wasm_path)
        .with_context(|| format!("WASM module failed to compile/validate: {wasm_path}"))?;
    validate_inspection(&inspection)?;

    match output {
        OutputFormat::Text => {
            eprintln!("[brrmmmm] validating {wasm_path}");
            eprintln!("[brrmmmm] ✓ WASM module validates successfully");
            eprintln!(
                "[brrmmmm]   entry: {}",
                inspection.entrypoint.as_deref().unwrap_or("unknown")
            );
            eprintln!("[brrmmmm]   ABI: v{}", inspection.abi_version);
            eprintln!("[brrmmmm]   size: {} bytes", inspection.wasm_size_bytes);
            if let Some(describe) = &inspection.describe {
                eprintln!(
                    "[brrmmmm]   contract: {} ({})",
                    describe.name, describe.logical_id
                );
                if !describe.run_modes.is_empty() {
                    eprintln!("[brrmmmm]   modes: {}", describe.run_modes.join(", "));
                }
            }
            if !inspection.brrmmmm_exports.is_empty() {
                eprintln!(
                    "[brrmmmm]   exports: {}",
                    inspection.brrmmmm_exports.join(", ")
                );
            }
        }
        OutputFormat::Json => {
            let d = inspection.describe.as_ref();
            let obj = serde_json::json!({
                "valid": true,
                "abi_version": inspection.abi_version,
                "size_bytes": inspection.wasm_size_bytes,
                "entrypoint": inspection.entrypoint,
                "name": d.map(|d| &d.name),
                "logical_id": d.map(|d| &d.logical_id),
                "modes": d.map(|d| &d.run_modes),
                "exports": inspection.brrmmmm_exports,
            });
            println!("{}", serde_json::to_string_pretty(&obj).unwrap());
        }
        OutputFormat::Table => {
            let d = inspection.describe.as_ref();
            let mut rows: Vec<(&str, String)> = vec![
                ("valid", "✓".to_string()),
                ("abi_version", inspection.abi_version.to_string()),
                ("size_bytes", inspection.wasm_size_bytes.to_string()),
                (
                    "entrypoint",
                    inspection.entrypoint.clone().unwrap_or_default(),
                ),
            ];
            if let Some(d) = d {
                rows.push(("name", d.name.clone()));
                rows.push(("logical_id", d.logical_id.clone()));
                if !d.run_modes.is_empty() {
                    rows.push(("modes", d.run_modes.join(", ")));
                }
            }
            if !inspection.brrmmmm_exports.is_empty() {
                rows.push(("exports", inspection.brrmmmm_exports.join(", ")));
            }
            print_table(&rows);
        }
    }

    Ok(())
}
