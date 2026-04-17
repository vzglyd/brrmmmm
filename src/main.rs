use std::io::{BufRead, Write};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

mod abi;
mod controller;
mod events;
mod host;
mod persistence;

use controller::{SidecarController, inspect_wasm_contract, validate_inspection};
use events::{EnvVarStatus, Event, EventSink, now_ts};

#[derive(Parser)]
#[command(
    name = "brrmmmm",
    about = "Standalone sidecar runner for VZGLYD sidecar WASM modules\n\nOpenAPI describes the endpoint. brrmmmm describes the behavior.",
    version
)]
struct Cli {
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

fn main() -> Result<()> {
    env_logger::init();
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
        ),
        Commands::Inspect { wasm_path } => cmd_inspect(&wasm_path),
        Commands::Validate { wasm_path } => cmd_validate(&wasm_path),
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
                    std::io::stdout().write_all(&data)?;
                    std::io::stdout().write_all(b"\n")?;
                }
                if !events_mode {
                    eprintln!(
                        "[brrmmmm] payload received ({} bytes), stopping",
                        data.len()
                    );
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

fn cmd_inspect(wasm_path: &str) -> Result<()> {
    eprintln!("[brrmmmm] inspecting {wasm_path}");

    let inspection = inspect_wasm_contract(wasm_path)?;
    println!("{}", serde_json::to_string_pretty(&inspection).unwrap());
    Ok(())
}

fn cmd_validate(wasm_path: &str) -> Result<()> {
    eprintln!("[brrmmmm] validating {wasm_path}");

    let inspection = inspect_wasm_contract(wasm_path)
        .with_context(|| format!("WASM module failed to compile/validate: {wasm_path}"))?;
    validate_inspection(&inspection)?;

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
        eprintln!("[brrmmmm]   exports: {}", inspection.brrmmmm_exports.join(", "));
    }

    Ok(())
}
