use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};
use std::time::Instant;

use anyhow::Result;
use wasmtime::Linker;

use crate::abi::{SidecarPhase, SidecarRuntimeState};
use crate::events::{Event, EventSink, diag, now_ts};
use crate::host::HostState;
use crate::host::host_request::{ErrorKind, HostRequest, HostResponse};

use super::super::super::io::{
    describe_request, encode_response_for_sidecar, execute_native_request, read_memory_from_caller,
    response_info, update_failure_state, update_phase_state,
};
use super::publish::publish_raw_source_payload;
use super::state::store_pending_response;

pub(super) fn register(
    linker: &mut Linker<wasmtime_wasi::preview1::WasiP1Ctx>,
    shared: Arc<Mutex<HostState>>,
    event_sink: EventSink,
    runtime_state: Arc<Mutex<SidecarRuntimeState>>,
    request_counter: Arc<AtomicU64>,
) -> Result<()> {
    linker.func_wrap(
        "vzglyd_host",
        "network_request",
        move |mut caller: wasmtime::Caller<'_, wasmtime_wasi::preview1::WasiP1Ctx>,
              ptr: i32,
              len: i32|
              -> i32 {
            let req_bytes = match read_memory_from_caller(&mut caller, ptr, len) {
                Ok(bytes) => bytes,
                Err(error) => {
                    diag(
                        &event_sink,
                        &format!("[brrmmmm] network_request memory error: {error}"),
                    );
                    return -1;
                }
            };

            let Some(request) = decode_request(&req_bytes, &event_sink) else {
                return -1;
            };

            let req_id = request_counter.fetch_add(1, Ordering::Relaxed);
            let request_id = format!("r{req_id}");
            let (req_kind, req_host, req_path) = describe_request(&request);
            update_phase_state(&runtime_state, &event_sink, SidecarPhase::Fetching);
            event_sink.emit(Event::RequestStart {
                ts: now_ts(),
                request_id: request_id.clone(),
                kind: req_kind,
                host: req_host,
                path: req_path,
            });

            let start = Instant::now();
            let response = match execute_native_request(&request) {
                Ok(response) => response,
                Err(error) => {
                    update_failure_state(&runtime_state, &error);
                    event_sink.emit(Event::RequestError {
                        ts: now_ts(),
                        request_id,
                        error_kind: "io".to_string(),
                        message: error.clone(),
                    });
                    let response = HostResponse::Error {
                        error_kind: ErrorKind::Io,
                        message: error,
                    };
                    store_pending_response(&shared, encode_response_for_sidecar(&response));
                    return 0;
                }
            };

            let elapsed_ms = start.elapsed().as_millis() as u64;
            let (status_code, response_size) = response_info(&response);
            event_sink.emit(Event::RequestDone {
                ts: now_ts(),
                request_id,
                status_code,
                elapsed_ms,
                response_size_bytes: response_size,
            });

            publish_raw_source_payload(&response, &shared, &runtime_state, &event_sink);
            store_pending_response(&shared, encode_response_for_sidecar(&response));
            0
        },
    )?;

    Ok(())
}

fn decode_request(req_bytes: &[u8], sink: &EventSink) -> Option<HostRequest> {
    let decoded: serde_json::Value = match serde_json::from_slice(req_bytes) {
        Ok(value) => value,
        Err(error) => {
            diag(
                sink,
                &format!("[brrmmmm] network_request decode error: {error}"),
            );
            return None;
        }
    };

    let wire_version = decoded
        .get("wire_version")
        .and_then(|value| value.as_u64())
        .unwrap_or(0) as u8;
    if wire_version != crate::host::host_request::WIRE_VERSION {
        diag(
            sink,
            &format!("[brrmmmm] network_request wire_version mismatch: {wire_version}"),
        );
        return None;
    }

    match serde_json::from_value(decoded) {
        Ok(request) => Some(request),
        Err(error) => {
            diag(
                sink,
                &format!("[brrmmmm] network_request parse error: {error}"),
            );
            None
        }
    }
}
