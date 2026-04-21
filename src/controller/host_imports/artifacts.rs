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
    let s_artifact = shared.clone();
    let sink_artifact = event_sink.clone();
    let runtime_artifact = runtime_state.clone();
    linker.func_wrap(
        "brrmmmm_host",
        "artifact_publish",
        move |mut caller: WasmCaller<'_>,
              kind_ptr: i32,
              kind_len: i32,
              data_ptr: i32,
              data_len: i32|
              -> i32 {
            let limits = lock_runtime(&s_artifact, "host_state")
                .config
                .limits
                .clone();
            let kind_bytes = match read_limited_memory_from_caller(
                &mut caller,
                kind_ptr,
                kind_len,
                256,
                "artifact kind",
            ) {
                Ok(b) => b,
                Err(_) => return -1,
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
                        &sink_artifact,
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

            {
                let hs = lock_runtime(&s_artifact, "host_state");
                if hs.log_channel && kind == "published_output" {
                    diag(
                        &sink_artifact,
                        &format!("[brrmmmm] published_output: {size} bytes"),
                    );
                    diag(
                        &sink_artifact,
                        &format!(
                            "[brrmmmm]   payload: {}",
                            &preview.chars().take(200).collect::<String>()
                        ),
                    );
                }
                lock_runtime(&*hs.artifact_store, "artifact_store").store(artifact);
            }

            update_artifact_state(&runtime_artifact, &meta);
            update_phase_state(&runtime_artifact, &sink_artifact, MissionPhase::Publishing);
            sink_artifact.emit(Event::ArtifactReceived {
                ts: now_ts(),
                kind,
                size_bytes: size,
                preview,
                artifact: meta,
            });
            0
        },
    )?;

    let sink_manifest = event_sink;
    let runtime_manifest = runtime_state;
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
                lock_runtime(&runtime_manifest, "runtime_state").describe = Some(describe.clone());
                sink_manifest.emit(Event::Describe {
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
