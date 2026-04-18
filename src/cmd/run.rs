use std::io::BufRead;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use anyhow::{Context, Result};

use brrmmmm::controller::SidecarController;
use brrmmmm::events::{self, EnvVarStatus, Event, EventSink, now_ts};
use brrmmmm::{params, persistence};

use crate::cli::OutputFormat;

use super::output::write_payload;

pub(crate) struct RunOptions<'a> {
    pub(crate) wasm_path: &'a str,
    pub(crate) once: bool,
    pub(crate) interval: u64,
    pub(crate) env: &'a [String],
    pub(crate) params_json: Option<&'a str>,
    pub(crate) params_file: Option<&'a str>,
    pub(crate) log_channel: bool,
    pub(crate) events_mode: bool,
    pub(crate) verbose: bool,
    pub(crate) output: OutputFormat,
}

pub(crate) fn cmd_run(options: RunOptions<'_>) -> Result<()> {
    let RunOptions {
        wasm_path,
        once,
        interval,
        env,
        params_json,
        params_file,
        log_channel,
        events_mode,
        verbose: _verbose,
        output,
    } = options;

    let env_vars = params::parse_env_vars(env);
    let params_bytes = params::parse_params_bytes(params_json, params_file)?;

    let sink = if events_mode {
        EventSink::for_stdout()
    } else {
        EventSink::noop()
    };

    if events_mode {
        sink.emit(Event::EnvSnapshot {
            ts: now_ts(),
            vars: EnvVarStatus::from_raw_env(&env_vars),
        });
    }

    if once {
        run_once(RunOnceOptions {
            wasm_path,
            env_vars,
            params_bytes,
            log_channel,
            events_mode,
            output,
            interval,
            sink,
        })
    } else {
        run_daemon(RunDaemonOptions {
            wasm_path,
            env_vars,
            params_bytes,
            log_channel,
            events_mode,
            interval,
            sink,
        })
    }
}

struct RunOnceOptions<'a> {
    wasm_path: &'a str,
    env_vars: Vec<(String, String)>,
    params_bytes: Option<Vec<u8>>,
    log_channel: bool,
    events_mode: bool,
    output: OutputFormat,
    interval: u64,
    sink: EventSink,
}

fn run_once(options: RunOnceOptions<'_>) -> Result<()> {
    let RunOnceOptions {
        wasm_path,
        env_vars,
        params_bytes,
        log_channel,
        events_mode,
        output,
        interval,
        sink,
    } = options;

    if !events_mode {
        eprintln!("[brrmmmm] running {wasm_path} in --once mode");
        eprintln!("[brrmmmm] starting sidecar, waiting for first channel_push...");
    }

    let controller =
        SidecarController::new(wasm_path, env_vars, params_bytes.clone(), log_channel, sink)
            .with_context(|| format!("failed to load sidecar: {wasm_path}"))?;

    let timeout = std::time::Duration::from_secs(std::cmp::max(interval * 2, 30));
    let start = std::time::Instant::now();

    loop {
        if let Some(data) = controller.poll_output() {
            if !events_mode {
                write_payload(&data, output)?;
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
}

struct RunDaemonOptions<'a> {
    wasm_path: &'a str,
    env_vars: Vec<(String, String)>,
    params_bytes: Option<Vec<u8>>,
    log_channel: bool,
    events_mode: bool,
    interval: u64,
    sink: EventSink,
}

fn run_daemon(options: RunDaemonOptions<'_>) -> Result<()> {
    let RunDaemonOptions {
        wasm_path,
        env_vars,
        params_bytes,
        log_channel,
        events_mode,
        interval,
        sink,
    } = options;

    if !events_mode {
        eprintln!("[brrmmmm] running {wasm_path} in daemon mode (interval: {interval}s)");
        eprintln!("[brrmmmm] sidecar controls polling; Ctrl+C to stop");
    }

    let wasm_bytes =
        std::fs::read(wasm_path).with_context(|| format!("read WASM file: {wasm_path}"))?;
    let wasm_hash = persistence::wasm_identity(&wasm_bytes);
    drop(wasm_bytes);

    let controller =
        SidecarController::new(wasm_path, env_vars, params_bytes, log_channel, sink.clone())
            .with_context(|| format!("failed to load sidecar: {wasm_path}"))?;

    if events_mode {
        spawn_command_listener(&controller, sink.clone());
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

    let state = controller.snapshot();
    persistence::save(&wasm_hash, &state);

    controller.stop();
    Ok(())
}

fn spawn_command_listener(controller: &SidecarController, sink: EventSink) {
    let flag = controller.force_refresh_flag();
    let params_handle = controller.params_handle();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines().map_while(std::result::Result::ok) {
            let trimmed = line.trim();
            if trimmed == "force" {
                flag.store(true, Ordering::Relaxed);
            } else if let Some(raw) = trimmed.strip_prefix("params_json ") {
                handle_params_json_command(raw, &params_handle, &flag, &sink);
            }
        }
    });
}

fn handle_params_json_command(
    raw: &str,
    params_handle: &Arc<Mutex<Option<Vec<u8>>>>,
    flag: &AtomicBool,
    sink: &EventSink,
) {
    match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(value) if value.is_object() => match serde_json::to_vec(&value) {
            Ok(bytes) => {
                replace_params(params_handle, bytes, sink);
                flag.store(true, Ordering::Relaxed);
                events::diag(
                    sink,
                    "[brrmmmm] updated sidecar params and requested refresh",
                );
            }
            Err(error) => events::diag(
                sink,
                &format!("[brrmmmm] failed to encode params_json: {error}"),
            ),
        },
        Ok(_) => events::diag(
            sink,
            "[brrmmmm] params_json command must contain a JSON object",
        ),
        Err(error) => events::diag(
            sink,
            &format!("[brrmmmm] invalid params_json command: {error}"),
        ),
    }
}

fn replace_params(params_handle: &Arc<Mutex<Option<Vec<u8>>>>, bytes: Vec<u8>, sink: &EventSink) {
    match params_handle.lock() {
        Ok(mut params) => *params = Some(bytes),
        Err(poisoned) => {
            events::diag(
                sink,
                "[brrmmmm] params mutex was poisoned; recovering with latest params",
            );
            *poisoned.into_inner() = Some(bytes);
        }
    }
}
