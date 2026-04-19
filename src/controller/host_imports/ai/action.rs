use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::Result;
use wasmtime::Linker;

use crate::events::{Event, EventSink, diag, now_ts};
use crate::host::HostState;
use crate::host::ai_request::{decode_action, encode_response};

use super::execute::AiSession;
use super::state::store_pending_response;

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

            let start = Instant::now();
            let response = {
                let sess = match session.lock() {
                    Ok(g) => g,
                    Err(p) => p.into_inner(),
                };
                sess.execute(action)
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
