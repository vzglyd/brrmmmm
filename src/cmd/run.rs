use anyhow::{Context, Result};
use std::path::Path;

use brrmmmm::abi::MissionOutcomeStatus;
use brrmmmm::config::Config;
use brrmmmm::controller::MissionController;
use brrmmmm::error::BrrmmmmError;
use brrmmmm::events::{EnvVarStatus, Event, EventSink, now_ts};

use crate::cli::OutputFormat;
use crate::mission_result::MissionRecorder;

use super::output::write_payload;

pub struct RunOptions<'a> {
    pub(crate) wasm_path: &'a Path,
    pub(crate) env_vars: Vec<(String, String)>,
    pub(crate) params_bytes: Option<Vec<u8>>,
    pub(crate) mission_recorder: Option<MissionRecorder>,
    pub(crate) log_channel: bool,
    pub(crate) events_mode: bool,
    pub(crate) override_retry_gate: bool,
    pub(crate) output: OutputFormat,
    pub(crate) config: &'a Config,
}

pub fn cmd_run(options: RunOptions<'_>) -> Result<()> {
    let RunOptions {
        wasm_path,
        env_vars,
        params_bytes,
        mission_recorder,
        log_channel,
        events_mode,
        override_retry_gate,
        output,
        config,
    } = options;

    let sink = if events_mode {
        EventSink::for_stdout()
    } else {
        EventSink::noop()
    };

    if events_mode {
        sink.emit(&Event::EnvSnapshot {
            ts: now_ts(),
            vars: EnvVarStatus::from_raw_env(&env_vars),
        });
    }

    run_once(RunOnceOptions {
        wasm_path,
        env_vars,
        params_bytes,
        mission_recorder,
        log_channel,
        events_mode,
        override_retry_gate,
        output,
        sink,
        config,
    })
}

struct RunOnceOptions<'a> {
    wasm_path: &'a Path,
    env_vars: Vec<(String, String)>,
    params_bytes: Option<Vec<u8>>,
    mission_recorder: Option<MissionRecorder>,
    log_channel: bool,
    events_mode: bool,
    override_retry_gate: bool,
    output: OutputFormat,
    sink: EventSink,
    config: &'a Config,
}

fn run_once(options: RunOnceOptions<'_>) -> Result<()> {
    const WATCHDOG_GRACE_MS: u64 = 250;

    let RunOnceOptions {
        wasm_path,
        env_vars,
        params_bytes,
        mission_recorder,
        log_channel,
        events_mode,
        override_retry_gate,
        output,
        sink,
        config,
    } = options;

    let wasm_str = wasm_path.to_string_lossy();

    if !events_mode {
        eprintln!("[brrmmmm] running {wasm_str} in --once mode");
        eprintln!("[brrmmmm] starting mission module, waiting for a terminal outcome...");
    }

    let controller = MissionController::new(
        &wasm_str,
        env_vars,
        params_bytes,
        log_channel,
        override_retry_gate,
        sink,
        config,
    )
    .with_context(|| format!("failed to load mission module: {wasm_str}"))
    .map_err(|error| record_failure(mission_recorder.as_ref(), error))?;

    let mut timeout = std::time::Duration::from_secs(30);
    let start = std::time::Instant::now();

    loop {
        if let Some(timeout_secs) = controller
            .acquisition_timeout_secs()
            .filter(|timeout_secs| *timeout_secs > 0)
        {
            timeout = std::time::Duration::from_secs(u64::from(timeout_secs));
        }

        if let Some(completion) = controller.poll_completion() {
            if let Some(recorder) = mission_recorder.as_ref() {
                recorder
                    .write_completion(&completion)
                    .map_err(|error| record_failure(mission_recorder.as_ref(), error))?;
            } else if !events_mode
                && completion.outcome.status == MissionOutcomeStatus::Published
                && let Some(data) = completion.published_output.as_deref()
            {
                write_payload(data, output)?;
                eprintln!(
                    "[brrmmmm] published output received ({} bytes), stopping",
                    data.len()
                );
            }
            controller.stop();
            return match completion.outcome.status {
                MissionOutcomeStatus::Published => Ok(()),
                MissionOutcomeStatus::RetryableFailure => {
                    if completion.outcome.reason_code == "acquisition_timeout" {
                        Err(BrrmmmmError::Timeout(completion.outcome.message).into())
                    } else {
                        Err(BrrmmmmError::RetryableFailure(completion.outcome.message).into())
                    }
                }
                MissionOutcomeStatus::TerminalFailure => {
                    Err(BrrmmmmError::RuntimeFailure(completion.outcome.message).into())
                }
                MissionOutcomeStatus::OperatorActionRequired => {
                    Err(BrrmmmmError::OperatorActionRequired(completion.outcome.message).into())
                }
            };
        }

        let snapshot = controller.snapshot();
        let remaining_cooldown_ms = snapshot.cooldown_until_ms.map_or(0, |until_ms| {
            until_ms.saturating_sub(brrmmmm::events::now_ms())
        });
        let watchdog_timeout = timeout.saturating_add(std::time::Duration::from_millis(
            remaining_cooldown_ms.saturating_add(WATCHDOG_GRACE_MS),
        ));

        if start.elapsed() > watchdog_timeout {
            if !events_mode {
                eprintln!(
                    "[brrmmmm] timeout waiting for a terminal mission outcome ({}s)",
                    timeout.as_secs()
                );
            }
            controller.stop();
            let error = BrrmmmmError::Timeout(format!(
                "waiting for mission outcome after {}s",
                timeout.as_secs()
            ));
            return Err(record_failure(mission_recorder.as_ref(), error.into()));
        }

        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

fn record_failure(recorder: Option<&MissionRecorder>, error: anyhow::Error) -> anyhow::Error {
    let Some(recorder) = recorder else {
        return error;
    };
    match recorder.write_runtime_error(&error) {
        Ok(()) => error,
        Err(write_error) => write_error.context(format!("original run error: {error:#}")),
    }
}
