use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::Result;
use wasmtime::Linker;

use crate::events::{Event, EventSink, diag, now_ts};
use crate::host::HostState;
use crate::host::browser_request::{BrowserActionResponse, decode_action, encode_response};
use crate::mission_state::{self, CAP_BROWSER};

use super::execute::BrowserSession;
use super::state::store_pending_response;

use super::super::super::io::lock_runtime;

pub(super) fn register(
    linker: &mut Linker<wasmtime_wasi::preview1::WasiP1Ctx>,
    shared: Arc<Mutex<HostState>>,
    event_sink: EventSink,
    session: Arc<Mutex<BrowserSession>>,
) -> Result<()> {
    linker.func_wrap(
        "vzglyd_host",
        "browser_action",
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
                        &format!("[brrmmmm] browser_action memory error: {e}"),
                    );
                    return -1;
                }
            };

            let action = match decode_action(&bytes) {
                Ok(a) => a,
                Err(e) => {
                    diag(
                        &event_sink,
                        &format!("[brrmmmm] browser_action decode error: {e}"),
                    );
                    let error_response =
                        BrowserActionResponse::err("decode_error", e.to_string());
                    if let Ok(data) = encode_response(&error_response) {
                        store_pending_response(&shared, data);
                        return 0;
                    }
                    return -1;
                }
            };

            let action_kind = action.kind().to_string();
            let action_detail = action.detail();
            {
                let mut host = lock_runtime(&shared, "host_state");
                let event = mission_state::browser_action_event(&action_kind);
                host.record_activity(CAP_BROWSER, "browser_action", &event);
            }

            event_sink.emit(Event::BrowserAction {
                ts: now_ts(),
                action: action_kind.clone(),
                detail: action_detail,
            });

            let start = Instant::now();
            let response = {
                let mut sess = match session.lock() {
                    Ok(g) => g,
                    Err(p) => p.into_inner(),
                };
                sess.execute(action)
            };
            let elapsed_ms = start.elapsed().as_millis() as u64;
            let ok = response.is_ok();
            let error = if ok { None } else { Some(action_kind.clone()) };

            event_sink.emit(Event::BrowserActionDone {
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
                        &format!("[brrmmmm] browser_action encode error: {e}"),
                    );
                    -1
                }
            }
        },
    )?;

    Ok(())
}
