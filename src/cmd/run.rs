use anyhow::{Context, Result};
use std::path::Path;

use brrmmmm::config::Config;
use brrmmmm::controller::SidecarController;
use brrmmmm::events::{EnvVarStatus, Event, EventSink, now_ts};

use crate::cli::OutputFormat;

use super::params;
use super::output::write_payload;

pub(crate) struct RunOptions<'a> {
    pub(crate) wasm_path: &'a Path,
    pub(crate) env: &'a [String],
    pub(crate) params_json: Option<&'a str>,
    pub(crate) params_file: Option<&'a Path>,
    pub(crate) log_channel: bool,
    pub(crate) events_mode: bool,
    pub(crate) verbose: bool,
    pub(crate) output: OutputFormat,
    pub(crate) config: &'a Config,
}

pub(crate) fn cmd_run(options: RunOptions<'_>) -> Result<()> {
    let RunOptions {
        wasm_path,
        env,
        params_json,
        params_file,
        log_channel,
        events_mode,
        verbose: _verbose,
        output,
        config,
    } = options;

    let env_vars = params::parse_env_vars(env);
    let params_bytes = params::parse_params_bytes(params_json, params_file, &config.limits)?;

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

    run_once(RunOnceOptions {
        wasm_path,
        env_vars,
        params_bytes,
        log_channel,
        events_mode,
        output,
        sink,
        config,
    })
}

struct RunOnceOptions<'a> {
    wasm_path: &'a Path,
    env_vars: Vec<(String, String)>,
    params_bytes: Option<Vec<u8>>,
    log_channel: bool,
    events_mode: bool,
    output: OutputFormat,
    sink: EventSink,
    config: &'a Config,
}

fn run_once(options: RunOnceOptions<'_>) -> Result<()> {
    let RunOnceOptions {
        wasm_path,
        env_vars,
        params_bytes,
        log_channel,
        events_mode,
        output,
        sink,
        config,
    } = options;

    let wasm_str = wasm_path.to_string_lossy();

    if !events_mode {
        eprintln!("[brrmmmm] running {wasm_str} in --once mode");
        eprintln!("[brrmmmm] starting sidecar, waiting for first channel_push...");
    }

    let controller = SidecarController::new(
        &wasm_str,
        env_vars,
        params_bytes.clone(),
        log_channel,
        sink,
        config,
    )
    .with_context(|| format!("failed to load sidecar: {wasm_str}"))?;

    let mut timeout = std::time::Duration::from_secs(30);
    let start = std::time::Instant::now();

    loop {
        if let Some(timeout_secs) = controller
            .acquisition_timeout_secs()
            .filter(|timeout_secs| *timeout_secs > 0)
        {
            timeout = std::time::Duration::from_secs(timeout_secs as u64);
        }

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
