use std::sync::{Arc, Mutex};

use crate::abi::{MissionOutcome, MissionOutcomeStatus};
use crate::events::{EventSink, diag};

use super::super::io::{
    WasmCaller, WasmLinker, lock_runtime, read_limited_memory_from_caller,
    update_mission_outcome_state,
};
use crate::abi::MissionRuntimeState;
use crate::host::HostState;

pub(super) fn register(
    linker: &mut WasmLinker,
    shared: Arc<Mutex<HostState>>,
    event_sink: EventSink,
    runtime_state: Arc<Mutex<MissionRuntimeState>>,
) -> anyhow::Result<()> {
    linker.func_wrap(
        "brrmmmm_host",
        "mission_outcome_report",
        move |mut caller: WasmCaller<'_>, ptr: i32, len: i32| -> i32 {
            let host_config = lock_runtime(&shared, "host_state").config.clone();
            let limits = host_config.limits.clone();
            let assurance = host_config.assurance;
            let payload = match read_limited_memory_from_caller(
                &mut caller,
                ptr,
                len,
                limits.max_host_payload_bytes,
                "mission outcome payload",
            ) {
                Ok(payload) => payload,
                Err(error) => {
                    return reject_outcome_report(
                        &runtime_state,
                        &event_sink,
                        &assurance,
                        &format!("[brrmmmm] mission_outcome_report memory error: {error}"),
                    );
                }
            };

            let outcome = match serde_json::from_slice::<MissionOutcome>(&payload) {
                Ok(outcome) => outcome,
                Err(error) => {
                    return reject_outcome_report(
                        &runtime_state,
                        &event_sink,
                        &assurance,
                        &format!("[brrmmmm] mission_outcome_report decode error: {error}"),
                    );
                }
            };

            if outcome.reason_code.trim().is_empty() || outcome.message.trim().is_empty() {
                return reject_outcome_report(
                    &runtime_state,
                    &event_sink,
                    &assurance,
                    "[brrmmmm] mission_outcome_report requires non-empty reason_code and message",
                );
            }

            match update_mission_outcome_state(
                &runtime_state,
                &event_sink,
                outcome,
                "module",
                &assurance,
            ) {
                Ok(()) => 0,
                Err(error) => reject_outcome_report(
                    &runtime_state,
                    &event_sink,
                    &assurance,
                    &format!("[brrmmmm] mission_outcome_report contract error: {error}"),
                ),
            }
        },
    )?;
    Ok(())
}

fn reject_outcome_report(
    runtime_state: &Arc<Mutex<MissionRuntimeState>>,
    event_sink: &EventSink,
    assurance: &crate::config::RuntimeAssurance,
    message: &str,
) -> i32 {
    diag(event_sink, message);
    let protocol_failure = MissionOutcome {
        status: MissionOutcomeStatus::TerminalFailure,
        reason_code: "mission_protocol_error".to_string(),
        message: message.to_string(),
        retry_after_ms: None,
        operator_action: None,
        operator_timeout_ms: None,
        operator_timeout_outcome: None,
        primary_artifact_kind: None,
    };
    if let Err(error) = update_mission_outcome_state(
        runtime_state,
        event_sink,
        protocol_failure,
        "host",
        assurance,
    )
    {
        diag(
            event_sink,
            &format!("[brrmmmm] failed to persist mission protocol error outcome: {error}"),
        );
    }
    -1
}
