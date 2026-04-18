use std::sync::{Arc, Mutex};

use crate::abi::{ArtifactMeta, SidecarDescribe, SidecarPhase, SidecarRuntimeState};
use crate::events::{Event, EventSink, diag, now_ms, now_ts};
use crate::host::{Artifact, HostState};

use super::super::io::{
    lock_runtime, read_memory_from_caller, update_artifact_state, update_phase_state,
};

pub(super) fn register(
    linker: &mut wasmtime::Linker<wasmtime_wasi::preview1::WasiP1Ctx>,
    shared: Arc<Mutex<HostState>>,
    event_sink: EventSink,
    runtime_state: Arc<Mutex<SidecarRuntimeState>>,
) -> anyhow::Result<()> {
    let s_push = shared.clone();
    let sink_push = event_sink.clone();
    let runtime_push = runtime_state.clone();
    linker.func_wrap(
        "vzglyd_host",
        "channel_push",
        move |mut caller: wasmtime::Caller<'_, wasmtime_wasi::preview1::WasiP1Ctx>,
              ptr: i32,
              len: i32|
              -> i32 {
            let data = match read_memory_from_caller(&mut caller, ptr, len) {
                Ok(d) => d,
                Err(e) => {
                    diag(
                        &sink_push,
                        &format!("[brrmmmm] channel_push memory error: {e}"),
                    );
                    return -1;
                }
            };
            let size = data.len();
            let received_at = now_ms();
            let preview = String::from_utf8_lossy(&data).into_owned();

            let artifact = Artifact {
                kind: "published_output".to_string(),
                data,
                received_at_ms: received_at,
            };
            let meta = ArtifactMeta {
                kind: "published_output".to_string(),
                size_bytes: size,
                received_at_ms: received_at,
            };

            {
                let guard = lock_runtime(&s_push, "host_state");
                if guard.log_channel {
                    diag(&sink_push, &format!("[brrmmmm] channel_push: {size} bytes"));
                    diag(
                        &sink_push,
                        &format!(
                            "[brrmmmm]   payload: {}",
                            &preview.chars().take(200).collect::<String>()
                        ),
                    );
                }
                lock_runtime(&*guard.artifact_store, "artifact_store").store(artifact);
            }

            update_artifact_state(&runtime_push, &meta);
            update_phase_state(&runtime_push, &sink_push, SidecarPhase::Publishing);
            sink_push.emit(Event::ArtifactReceived {
                ts: now_ts(),
                kind: "published_output".to_string(),
                size_bytes: size,
                preview,
                artifact: meta,
            });
            0
        },
    )?;

    linker.func_wrap(
        "vzglyd_host",
        "channel_poll",
        |_caller: wasmtime::Caller<'_, wasmtime_wasi::preview1::WasiP1Ctx>,
         _ptr: i32,
         _len: i32|
         -> i32 { -1 },
    )?;

    linker.func_wrap(
        "vzglyd_host",
        "channel_active",
        |_caller: wasmtime::Caller<'_, wasmtime_wasi::preview1::WasiP1Ctx>| -> i32 { 1 },
    )?;

    let s_artifact = shared.clone();
    let sink_artifact = event_sink.clone();
    let runtime_artifact = runtime_state.clone();
    linker.func_wrap(
        "vzglyd_host",
        "artifact_publish",
        move |mut caller: wasmtime::Caller<'_, wasmtime_wasi::preview1::WasiP1Ctx>,
              kind_ptr: i32,
              kind_len: i32,
              data_ptr: i32,
              data_len: i32|
              -> i32 {
            let kind_bytes = match read_memory_from_caller(&mut caller, kind_ptr, kind_len) {
                Ok(b) => b,
                Err(_) => return -1,
            };
            let kind = String::from_utf8_lossy(&kind_bytes).into_owned();

            let data = match read_memory_from_caller(&mut caller, data_ptr, data_len) {
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
            let preview = String::from_utf8_lossy(&data).into_owned();

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
                lock_runtime(&*hs.artifact_store, "artifact_store").store(artifact);
            }

            update_artifact_state(&runtime_artifact, &meta);
            update_phase_state(&runtime_artifact, &sink_artifact, SidecarPhase::Publishing);
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
        "vzglyd_host",
        "register_manifest",
        move |mut caller: wasmtime::Caller<'_, wasmtime_wasi::preview1::WasiP1Ctx>,
              ptr: i32,
              len: i32|
              -> i32 {
            if let Ok(data) = read_memory_from_caller(&mut caller, ptr, len)
                && let Ok(describe) = serde_json::from_slice::<SidecarDescribe>(&data)
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
