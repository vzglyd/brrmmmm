use std::io::Write;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

mod host;
mod runner;

#[derive(Parser)]
#[command(
    name = "brrmmmm",
    about = "Standalone sidecar runner for VZGLYD sidecar WASM modules",
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

        /// Verbose output
        #[arg(short, long)]
        verbose: bool,
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
            verbose,
        } => cmd_run(&wasm_path, once, interval, &env, log_channel, verbose),
        Commands::Validate { wasm_path } => cmd_validate(&wasm_path),
    }
}

fn parse_env_vars(raw: &[String]) -> Vec<(String, String)> {
    raw.iter()
        .filter_map(|s| {
            s.split_once('=').map(|(k, v)| (k.to_string(), v.to_string()))
        })
        .collect()
}

fn cmd_run(
    wasm_path: &str,
    once: bool,
    interval: u64,
    env: &[String],
    log_channel: bool,
    _verbose: bool,
) -> Result<()> {
    let env_vars = parse_env_vars(env);

    if once {
        eprintln!("[brrmmmm] running {wasm_path} in --once mode");
        eprintln!("[brrmmmm] starting sidecar, waiting for first channel_push...");

        let runner = runner::SidecarRunner::new(wasm_path, env_vars, log_channel)
            .with_context(|| format!("failed to load sidecar: {wasm_path}"))?;

        // Wait for the first channel push with a timeout
        let timeout = std::time::Duration::from_secs(std::cmp::max(interval * 2, 30));
        let start = std::time::Instant::now();

        loop {
            if let Some(data) = runner.poll_channel() {
                // Print to stdout for piping
                std::io::stdout().write_all(&data)?;
                std::io::stdout().write_all(b"\n")?;
                eprintln!("[brrmmmm] payload received ({} bytes), stopping", data.len());
                runner.stop();
                return Ok(());
            }

            if start.elapsed() > timeout {
                eprintln!("[brrmmmm] timeout waiting for channel_push ({}s)", timeout.as_secs());
                runner.stop();
                anyhow::bail!("timeout waiting for sidecar payload");
            }

            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    } else {
        eprintln!("[brrmmmm] running {wasm_path} in daemon mode (interval: {interval}s)");
        eprintln!("[brrmmmm] sidecar controls polling; payloads will be logged");

        let runner = runner::SidecarRunner::new(wasm_path, env_vars, log_channel)
            .with_context(|| format!("failed to load sidecar: {wasm_path}"))?;

        // Wait for Ctrl+C
        let (tx, rx) = std::sync::mpsc::channel();
        ctrlc::set_handler(move || {
            let _ = tx.send(());
        })
        .context("set Ctrl+C handler")?;

        eprintln!("[brrmmmm] press Ctrl+C to stop");
        rx.recv().ok();

        eprintln!("\n[brrmmmm] stopping...");
        runner.stop();
        Ok(())
    }
}

fn cmd_validate(wasm_path: &str) -> Result<()> {
    eprintln!("[brrmmmm] validating {wasm_path}");

    let wasm_bytes = std::fs::read(wasm_path)
        .with_context(|| format!("read WASM file: {wasm_path}"))?;

    let engine = wasmtime::Engine::default();
    let module = wasmtime::Module::from_binary(&engine, &wasm_bytes)
        .context("WASM module failed to compile/validate")?;

    // Check for expected exports
    let has_start = module.get_export("_start").is_some();
    let has_main = module.get_export("main").is_some();

    if !has_start && !has_main {
        eprintln!("[brrmmmm] WARNING: no _start or main export found");
    }

    eprintln!("[brrmmmm] ✓ WASM module validates successfully");
    eprintln!("[brrmmmm]   entry: {}", if has_start { "_start" } else { "main" });
    eprintln!("[brrmmmm]   size: {} bytes", wasm_bytes.len());

    // List exports
    let exports: Vec<_> = module
        .exports()
        .filter_map(|e| {
            if e.name().starts_with('_') || e.name() == "main" || e.name().contains("fetch") {
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
