use std::sync::{Arc, Mutex};

use crate::abi::{ArtifactMeta, MissionModuleDescribe, MissionPhase, MissionRuntimeState};
use crate::events::{Event, EventSink, diag, now_ms, now_ts};
use crate::host::{Artifact, HostState};

use super::super::io::{
    WasmCaller, WasmLinker, lock_runtime, read_limited_memory_from_caller, update_artifact_state,
    update_phase_state,
};

pub(super) fn register(
    linker: &mut WasmLinker,
    shared: Arc<Mutex<HostState>>,
    event_sink: EventSink,
    runtime_state: Arc<Mutex<MissionRuntimeState>>,
) -> anyhow::Result<()> {
    register_artifact_publish(linker, shared, event_sink.clone(), runtime_state.clone())?;
    register_manifest(linker, event_sink, runtime_state)
}

fn register_artifact_publish(
    linker: &mut WasmLinker,
    shared: Arc<Mutex<HostState>>,
    event_sink: EventSink,
    runtime_state: Arc<Mutex<MissionRuntimeState>>,
) -> anyhow::Result<()> {
    linker.func_wrap(
        "brrmmmm_host",
        "artifact_publish",
        move |mut caller: WasmCaller<'_>,
              kind_ptr: i32,
              kind_len: i32,
              data_ptr: i32,
              data_len: i32|
              -> i32 {
            let limits = lock_runtime(&shared, "host_state").config.limits.clone();
            let Ok(kind_bytes) = read_limited_memory_from_caller(
                &mut caller,
                kind_ptr,
                kind_len,
                256,
                "artifact kind",
            ) else {
                return -1;
            };
            let kind = String::from_utf8_lossy(&kind_bytes).into_owned();

            let data = match read_limited_memory_from_caller(
                &mut caller,
                data_ptr,
                data_len,
                limits.max_artifact_bytes,
                "artifact payload",
            ) {
                Ok(d) => d,
                Err(e) => {
                    diag(
                        &event_sink,
                        &format!("[brrmmmm] artifact_publish memory error: {e}"),
                    );
                    return -1;
                }
            };
            let size = data.len();
            let received_at = now_ms();
            let preview = preview_string(&data, limits.max_artifact_preview_chars);

            let artifact = Artifact {
                kind: kind.clone(),
                data,
                received_at_ms: received_at,
            };
            let meta = ArtifactMeta {
                kind: kind.clone(),
                size_bytes: size,
                received_at_ms: received_at,
            };

            let artifact_store = {
                let hs = lock_runtime(&shared, "host_state");
                if hs.log_channel && kind == "published_output" {
                    diag(
                        &event_sink,
                        &format!("[brrmmmm] published_output: {size} bytes"),
                    );
                    diag(
                        &event_sink,
                        &format!(
                            "[brrmmmm]   payload: {}",
                            &preview.chars().take(200).collect::<String>()
                        ),
                    );
                }
                hs.artifact_store.clone()
            };
            lock_runtime(&artifact_store, "artifact_store").store(artifact);

            update_artifact_state(&runtime_state, &meta);
            update_phase_state(&runtime_state, &event_sink, MissionPhase::Publishing);
            event_sink.emit(&Event::ArtifactReceived {
                ts: now_ts(),
                kind,
                size_bytes: size,
                preview,
                artifact: meta,
            });
            0
        },
    )?;
    Ok(())
}

fn register_manifest(
    linker: &mut WasmLinker,
    event_sink: EventSink,
    runtime_state: Arc<Mutex<MissionRuntimeState>>,
) -> anyhow::Result<()> {
    linker.func_wrap(
        "brrmmmm_host",
        "register_manifest",
        move |mut caller: WasmCaller<'_>, ptr: i32, len: i32| -> i32 {
            if let Ok(data) = read_limited_memory_from_caller(
                &mut caller,
                ptr,
                len,
                1024 * 1024,
                "mission manifest",
            ) && let Ok(describe) = serde_json::from_slice::<MissionModuleDescribe>(&data)
            {
                lock_runtime(&runtime_state, "runtime_state").describe = Some(describe.clone());
                event_sink.emit(&Event::Describe {
                    ts: now_ts(),
                    describe,
                });
            }
            0
        },
    )?;
    Ok(())
}

fn preview_string(data: &[u8], max_chars: usize) -> String {
    String::from_utf8_lossy(data)
        .chars()
        .take(max_chars)
        .collect()
}
