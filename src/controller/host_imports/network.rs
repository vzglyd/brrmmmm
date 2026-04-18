use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};
use std::time::Instant;

use crate::abi::{ArtifactMeta, SidecarPhase, SidecarRuntimeState};
use crate::events::{Event, EventSink, diag, now_ms, now_ts};
use crate::host::host_request::{ErrorKind, HostResponse};
use crate::host::{Artifact, HostState};

use super::super::io::{
    describe_request, encode_response_for_sidecar, execute_native_request, lock_runtime,
    read_memory_from_caller, response_info, update_artifact_state, update_failure_state,
    update_phase_state, write_memory_from_caller,
};

pub(super) fn register(
    linker: &mut wasmtime::Linker<wasmtime_wasi::preview1::WasiP1Ctx>,
    shared: Arc<Mutex<HostState>>,
    event_sink: EventSink,
    runtime_state: Arc<Mutex<SidecarRuntimeState>>,
    request_counter: Arc<AtomicU64>,
) -> anyhow::Result<()> {
    let s_net = shared.clone();
    let sink_net = event_sink.clone();
    let counter_net = request_counter;
    let runtime_net = runtime_state.clone();
    linker.func_wrap(
        "vzglyd_host",
        "network_request",
        move |mut caller: wasmtime::Caller<'_, wasmtime_wasi::preview1::WasiP1Ctx>,
              ptr: i32,
              len: i32|
              -> i32 {
            let req_bytes = match read_memory_from_caller(&mut caller, ptr, len) {
                Ok(b) => b,
                Err(e) => {
                    diag(
                        &sink_net,
                        &format!("[brrmmmm] network_request memory error: {e}"),
                    );
                    return -1;
                }
            };

            let decoded: serde_json::Value = match serde_json::from_slice(&req_bytes) {
                Ok(v) => v,
                Err(e) => {
                    diag(
                        &sink_net,
                        &format!("[brrmmmm] network_request decode error: {e}"),
                    );
                    return -1;
                }
            };

            let wire_version = decoded
                .get("wire_version")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u8;
            if wire_version != crate::host::host_request::WIRE_VERSION {
                diag(
                    &sink_net,
                    &format!("[brrmmmm] network_request wire_version mismatch: {wire_version}"),
                );
                return -1;
            }

            let request: crate::host::host_request::HostRequest =
                match serde_json::from_value(decoded) {
                    Ok(r) => r,
                    Err(e) => {
                        diag(
                            &sink_net,
                            &format!("[brrmmmm] network_request parse error: {e}"),
                        );
                        return -1;
                    }
                };

            let req_id = counter_net.fetch_add(1, Ordering::Relaxed);
            let request_id = format!("r{req_id}");
            let (req_kind, req_host, req_path) = describe_request(&request);
            update_phase_state(&runtime_net, &sink_net, SidecarPhase::Fetching);
            sink_net.emit(Event::RequestStart {
                ts: now_ts(),
                request_id: request_id.clone(),
                kind: req_kind,
                host: req_host,
                path: req_path,
            });

            let start = Instant::now();
            let response = match execute_native_request(&request) {
                Ok(resp) => resp,
                Err(e) => {
                    update_failure_state(&runtime_net, &e);
                    sink_net.emit(Event::RequestError {
                        ts: now_ts(),
                        request_id: request_id.clone(),
                        error_kind: "io".to_string(),
                        message: e.clone(),
                    });
                    let response = HostResponse::Error {
                        error_kind: ErrorKind::Io,
                        message: e,
                    };
                    let resp_bytes = encode_response_for_sidecar(&response);
                    let guard = lock_runtime(&s_net, "host_state");
                    *lock_runtime(&*guard.pending_response, "pending_response") = Some(resp_bytes);
                    return 0;
                }
            };

            let elapsed_ms = start.elapsed().as_millis() as u64;

            let (status_code, response_size) = response_info(&response);
            sink_net.emit(Event::RequestDone {
                ts: now_ts(),
                request_id,
                status_code,
                elapsed_ms,
                response_size_bytes: response_size,
            });

            if let HostResponse::Http {
                status_code, body, ..
            } = &response
                && *status_code < 400
            {
                let received_at_ms = now_ms();
                let preview = String::from_utf8_lossy(body).into_owned();
                let raw_artifact = Artifact {
                    kind: "raw_source_payload".to_string(),
                    data: body.clone(),
                    received_at_ms,
                };
                {
                    let hs = lock_runtime(&s_net, "host_state");
                    lock_runtime(&*hs.artifact_store, "artifact_store").store(raw_artifact);
                }
                let meta = ArtifactMeta {
                    kind: "raw_source_payload".to_string(),
                    size_bytes: body.len(),
                    received_at_ms,
                };
                update_artifact_state(&runtime_net, &meta);
                sink_net.emit(Event::ArtifactReceived {
                    ts: now_ts(),
                    kind: "raw_source_payload".to_string(),
                    size_bytes: body.len(),
                    preview,
                    artifact: meta,
                });
            }

            let resp_bytes = encode_response_for_sidecar(&response);
            let guard = lock_runtime(&s_net, "host_state");
            *lock_runtime(&*guard.pending_response, "pending_response") = Some(resp_bytes);
            0
        },
    )?;

    let s_resp = shared.clone();
    linker.func_wrap(
        "vzglyd_host",
        "network_response_len",
        move |_caller: wasmtime::Caller<'_, wasmtime_wasi::preview1::WasiP1Ctx>| -> i32 {
            let guard = lock_runtime(&s_resp, "host_state");
            lock_runtime(&*guard.pending_response, "pending_response")
                .as_ref()
                .map(|b| b.len() as i32)
                .unwrap_or(-1)
        },
    )?;

    let s_read = shared;
    linker.func_wrap(
        "vzglyd_host",
        "network_response_read",
        move |mut caller: wasmtime::Caller<'_, wasmtime_wasi::preview1::WasiP1Ctx>,
              ptr: i32,
              len: i32|
              -> i32 {
            let guard = lock_runtime(&s_read, "host_state");
            let mut resp_guard = lock_runtime(&*guard.pending_response, "pending_response");
            let Some(data) = resp_guard.take() else {
                return -1;
            };
            let write_len = std::cmp::min(data.len(), len as usize);
            if let Err(e) = write_memory_from_caller(&mut caller, ptr, &data[..write_len]) {
                eprintln!("[brrmmmm] network_response_read error: {e}");
                return -1;
            }
            write_len as i32
        },
    )?;

    Ok(())
}
