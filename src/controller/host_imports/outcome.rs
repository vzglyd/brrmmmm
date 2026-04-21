use std::sync::{Arc, Mutex};

use crate::abi::MissionOutcome;
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
            let limits = lock_runtime(&shared, "host_state").config.limits.clone();
            let payload = match read_limited_memory_from_caller(
                &mut caller,
                ptr,
                len,
                limits.max_host_payload_bytes,
                "mission outcome payload",
            ) {
                Ok(payload) => payload,
                Err(error) => {
                    diag(
                        &event_sink,
                        &format!("[brrmmmm] mission_outcome_report memory error: {error}"),
                    );
                    return -1;
                }
            };

            let outcome = match serde_json::from_slice::<MissionOutcome>(&payload) {
                Ok(outcome) => outcome,
                Err(error) => {
                    diag(
                        &event_sink,
                        &format!("[brrmmmm] mission_outcome_report decode error: {error}"),
                    );
                    return -1;
                }
            };

            if outcome.reason_code.trim().is_empty() || outcome.message.trim().is_empty() {
                diag(
                    &event_sink,
                    "[brrmmmm] mission_outcome_report requires non-empty reason_code and message",
                );
                return -1;
            }

            update_mission_outcome_state(&runtime_state, &event_sink, outcome, "module");
            0
        },
    )?;
    Ok(())
}
