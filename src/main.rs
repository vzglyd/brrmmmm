use std::io::{BufRead, Write};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

mod abi;
mod controller;
mod events;
mod host;
mod persistence;

use controller::SidecarController;
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
            log_channel,
            events,
            verbose,
        } => cmd_run(&wasm_path, once, interval, &env, log_channel, events, verbose),
        Commands::Inspect { wasm_path } => cmd_inspect(&wasm_path),
        Commands::Validate { wasm_path } => cmd_validate(&wasm_path),
    }
}

fn parse_env_vars(raw: &[String]) -> Vec<(String, String)> {
    raw.iter()
        .filter_map(|s| s.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
        .collect()
}

fn cmd_run(
    wasm_path: &str,
    once: bool,
    interval: u64,
    env: &[String],
    log_channel: bool,
    events_mode: bool,
    _verbose: bool,
) -> Result<()> {
    let env_vars = parse_env_vars(env);

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

        let controller =
            SidecarController::new(wasm_path, env_vars, log_channel, sink.clone())
                .with_context(|| format!("failed to load sidecar: {wasm_path}"))?;

        let timeout =
            std::time::Duration::from_secs(std::cmp::max(interval * 2, 30));
        let start = std::time::Instant::now();

        loop {
            if let Some(data) = controller.poll_output() {
                std::io::stdout().write_all(&data)?;
                std::io::stdout().write_all(b"\n")?;
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
            eprintln!(
                "[brrmmmm] running {wasm_path} in daemon mode (interval: {interval}s)"
            );
            eprintln!("[brrmmmm] sidecar controls polling; Ctrl+C to stop");
        }

        // Compute WASM hash for state persistence.
        let wasm_bytes = std::fs::read(wasm_path)
            .with_context(|| format!("read WASM file: {wasm_path}"))?;
        let wasm_hash = persistence::wasm_identity(&wasm_bytes);
        drop(wasm_bytes);

        let controller =
            SidecarController::new(wasm_path, env_vars, log_channel, sink)
                .with_context(|| format!("failed to load sidecar: {wasm_path}"))?;

        // In events mode, listen on stdin for TUI commands ("force\n" = skip next sleep).
        if events_mode {
            let flag = controller.force_refresh_flag();
            std::thread::spawn(move || {
                let stdin = std::io::stdin();
                for line in stdin.lock().lines().map_while(Result::ok) {
                    if line.trim() == "force" {
                        flag.store(true, std::sync::atomic::Ordering::Relaxed);
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

    let wasm_bytes = std::fs::read(wasm_path)
        .with_context(|| format!("read WASM file: {wasm_path}"))?;

    let engine = wasmtime::Engine::default();
    let module = wasmtime::Module::from_binary(&engine, &wasm_bytes)
        .context("WASM module failed to compile")?;

    // Detect ABI version.
    let abi_version = if module
        .exports()
        .any(|e| e.name() == "vzglyd_sidecar_abi_version")
    {
        abi::ABI_VERSION_V2
    } else {
        abi::ABI_VERSION_V1
    };

    // Collect exports relevant to brrmmmm.
    let brrmmmm_exports: Vec<_> = module
        .exports()
        .filter(|e| {
            let n = e.name();
            n.starts_with("vzglyd_") || n == "_start" || n == "main"
        })
        .map(|e| e.name().to_string())
        .collect();

    let contract = serde_json::json!({
        "wasm_path": wasm_path,
        "wasm_size_bytes": wasm_bytes.len(),
        "abi_version": abi_version,
        "brrmmmm_exports": brrmmmm_exports,
        "note": if abi_version == abi::ABI_VERSION_V2 {
            "v2 sidecar: describe() blob available after instantiation (Sprint 2)"
        } else {
            "v1 sidecar: no self-description; behavior inferred from poll_loop"
        }
    });

    println!("{}", serde_json::to_string_pretty(&contract).unwrap());
    Ok(())
}

fn cmd_validate(wasm_path: &str) -> Result<()> {
    eprintln!("[brrmmmm] validating {wasm_path}");

    let wasm_bytes = std::fs::read(wasm_path)
        .with_context(|| format!("read WASM file: {wasm_path}"))?;

    let engine = wasmtime::Engine::default();
    let module = wasmtime::Module::from_binary(&engine, &wasm_bytes)
        .context("WASM module failed to compile/validate")?;

    let has_start = module.get_export("_start").is_some();
    let has_main = module.get_export("main").is_some();

    if !has_start && !has_main {
        eprintln!("[brrmmmm] WARNING: no _start or main export found");
    }

    eprintln!("[brrmmmm] ✓ WASM module validates successfully");
    eprintln!(
        "[brrmmmm]   entry: {}",
        if has_start { "_start" } else { "main" }
    );
    eprintln!("[brrmmmm]   size: {} bytes", wasm_bytes.len());

    let exports: Vec<_> = module
        .exports()
        .filter_map(|e| {
            if e.name().starts_with('_')
                || e.name() == "main"
                || e.name().contains("fetch")
                || e.name().starts_with("vzglyd_")
            {
                Some(e.name())
            } else {
                None
            }
        })
        .collect();

    if !exports.is_empty() {
        eprintln!("[brrmmmm]   exports: {}", exports.join(", "));
    }

    Ok(())
}
