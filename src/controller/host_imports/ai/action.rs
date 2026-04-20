use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::Result;
use wasmtime::Linker;

use crate::attestation::{self, RequestBinding};
use crate::events::{Event, EventSink, diag, now_ts};
use crate::host::HostState;
use crate::host::ai_request::{AiActionResponse, decode_action, encode_response};
use crate::mission_state::{self, CAP_AI};

use super::execute::AiSession;
use super::state::store_pending_response;

use super::super::super::io::lock_runtime;

pub(super) fn register(
    linker: &mut Linker<wasmtime_wasi::preview1::WasiP1Ctx>,
    shared: Arc<Mutex<HostState>>,
    event_sink: EventSink,
    session: Arc<Mutex<AiSession>>,
) -> Result<()> {
    linker.func_wrap(
        "vzglyd_host",
        "ai_request",
        move |mut caller: wasmtime::Caller<'_, wasmtime_wasi::preview1::WasiP1Ctx>,
              ptr: i32,
              len: i32|
              -> i32 {
            use super::super::super::io::read_memory_from_caller;

            let bytes = match read_memory_from_caller(&mut caller, ptr, len) {
                Ok(b) => b,
                Err(e) => {
                    diag(
                        &event_sink,
                        &format!("[brrmmmm] ai_request memory error: {e}"),
                    );
                    return -1;
                }
            };

            let action = match decode_action(&bytes) {
                Ok(a) => a,
                Err(e) => {
                    diag(
                        &event_sink,
                        &format!("[brrmmmm] ai_request decode error: {e}"),
                    );
                    let error_response = AiActionResponse::err("decode_error", e.to_string());
                    if let Ok(data) = encode_response(&error_response) {
                        store_pending_response(&shared, data);
                        return 0;
                    }
                    return -1;
                }
            };

            let action_kind = action.kind().to_string();
            let prompt_len = action.prompt_len();

            event_sink.emit(Event::AiRequest {
                ts: now_ts(),
                action: action_kind.clone(),
                prompt_len,
            });

            let body = {
                let sess = match session.lock() {
                    Ok(g) => g,
                    Err(p) => p.into_inner(),
                };
                sess.prepare_body(&action)
            };

            let body = match body {
                Ok(body) => body,
                Err(response) => {
                    {
                        let mut host = lock_runtime(&shared, "host_state");
                        let event = mission_state::ai_event(&action_kind);
                        host.record_activity(CAP_AI, "ai", &event);
                    }
                    return store_response_and_return(
                        &shared,
                        &event_sink,
                        &response,
                        action_kind,
                        0,
                    );
                }
            };

            let content_digest = attestation::sha256_digest(&body);
            let binding = RequestBinding::new(
                "POST",
                "api.anthropic.com",
                "/v1/messages",
                Some(content_digest),
            );
            let (ua, attestation_headers) = {
                let mut host = lock_runtime(&shared, "host_state");
                let event = mission_state::ai_event(&action_kind);
                let envelope = host.signed_envelope_for_request(CAP_AI, "ai", &event, &binding);
                let ua = host.full_user_agent(envelope.as_ref());
                let headers = envelope
                    .map(|envelope| envelope.headers)
                    .unwrap_or_default();
                (ua, headers)
            };

            let start = Instant::now();
            let response = {
                let sess = match session.lock() {
                    Ok(g) => g,
                    Err(p) => p.into_inner(),
                };
                sess.execute_prepared(body, ua, attestation_headers)
            };
            let elapsed_ms = start.elapsed().as_millis() as u64;
            let ok = response.is_ok();
            let error = response.error_code().map(ToOwned::to_owned);

            event_sink.emit(Event::AiRequestDone {
                ts: now_ts(),
                action: action_kind,
                elapsed_ms,
                ok,
                error,
            });

            match encode_response(&response) {
                Ok(data) => {
                    store_pending_response(&shared, data);
                    0
                }
                Err(e) => {
                    diag(
                        &event_sink,
                        &format!("[brrmmmm] ai_request encode error: {e}"),
                    );
                    -1
                }
            }
        },
    )?;

    Ok(())
}

fn store_response_and_return(
    shared: &Arc<Mutex<HostState>>,
    event_sink: &EventSink,
    response: &crate::host::ai_request::AiActionResponse,
    action_kind: String,
    elapsed_ms: u64,
) -> i32 {
    let ok = response.is_ok();
    let error = response.error_code().map(ToOwned::to_owned);
    event_sink.emit(Event::AiRequestDone {
        ts: now_ts(),
        action: action_kind,
        elapsed_ms,
        ok,
        error,
    });
    match encode_response(response) {
        Ok(data) => {
            store_pending_response(shared, data);
            0
        }
        Err(e) => {
            diag(
                event_sink,
                &format!("[brrmmmm] ai_request encode error: {e}"),
            );
            -1
        }
    }
}
