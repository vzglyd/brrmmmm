use std::future::Future;
use std::sync::{Arc, Mutex, atomic::AtomicU64};

use anyhow::Result;

use crate::abi::SidecarRuntimeState;
use crate::events::{EventSink, diag};
use crate::host::HostState;
use crate::host::host_call::{HostCall, decode_call, encode_result};

use super::super::io::{
    WasmCaller, WasmLinker, lock_runtime, read_limited_memory_from_caller, write_memory_from_caller,
};
use super::ai::SharedAiSession;
use super::browser::SharedBrowserSession;
use super::network::NetworkSession;

#[allow(clippy::too_many_arguments)]
pub(super) fn register(
    linker: &mut WasmLinker,
    shared: Arc<Mutex<HostState>>,
    event_sink: EventSink,
    runtime_state: Arc<Mutex<SidecarRuntimeState>>,
    request_counter: Arc<AtomicU64>,
    network: Arc<NetworkSession>,
    browser: SharedBrowserSession,
    ai: SharedAiSession,
) -> Result<()> {
    let call_shared = shared.clone();
    let call_sink = event_sink.clone();
    let call_runtime = runtime_state.clone();
    let call_counter = request_counter.clone();
    let call_network = network.clone();
    let call_browser = browser.clone();
    let call_ai = ai.clone();
    linker.func_wrap_async(
        "vzglyd_host",
        "host_call",
        move |mut caller: WasmCaller<'_>, (ptr, len): (i32, i32)| {
            let shared = call_shared.clone();
            let event_sink = call_sink.clone();
            let runtime_state = call_runtime.clone();
            let request_counter = call_counter.clone();
            let network = call_network.clone();
            let browser = call_browser.clone();
            let ai = call_ai.clone();

            let result = {
                let limits = lock_runtime(&shared, "host_state").config.limits.clone();
                read_limited_memory_from_caller(
                    &mut caller,
                    ptr,
                    len,
                    limits.max_host_payload_bytes,
                    "host_call payload",
                )
            };

            Box::new(async move {
                let bytes = match result {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        diag(
                            &event_sink,
                            &format!("[brrmmmm] host_call memory error: {error}"),
                        );
                        return -1;
                    }
                };

                let decoded = decode_call(&bytes);
                let capability = match decoded.as_ref() {
                    Ok(call) => call.capability(),
                    Err(_) => "host",
                };
                let response = match decoded {
                    Ok(call) => {
                        dispatch(
                            call,
                            &shared,
                            &event_sink,
                            &runtime_state,
                            &request_counter,
                            &network,
                            &browser,
                            &ai,
                        )
                        .await
                    }
                    Err(error) => Err(crate::host::host_call::HostCallError::new(
                        "decode_error",
                        error.to_string(),
                    )),
                };
                match encode_result(capability, response) {
                    Ok(data) => {
                        *pending_response_handle(&shared)
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(data);
                        0
                    }
                    Err(error) => {
                        diag(
                            &event_sink,
                            &format!("[brrmmmm] host_call encode error: {error}"),
                        );
                        -1
                    }
                }
            }) as Box<dyn Future<Output = i32> + Send>
        },
    )?;

    let len_shared = shared.clone();
    linker.func_wrap(
        "vzglyd_host",
        "host_response_len",
        move |_caller: WasmCaller<'_>| -> i32 { pending_response_len(&len_shared).unwrap_or(0) },
    )?;

    let read_shared = shared.clone();
    linker.func_wrap(
        "vzglyd_host",
        "host_response_read",
        move |mut caller: WasmCaller<'_>, ptr: i32, len: i32| -> i32 {
            let Some(data_len) = pending_response_len(&read_shared) else {
                return -1;
            };
            if len != data_len {
                return -1;
            }
            let Some(data) = take_pending_response(&read_shared) else {
                return -1;
            };
            if let Err(error) = write_memory_from_caller(&mut caller, ptr, &data) {
                eprintln!("[brrmmmm] host_response_read error: {error}");
                *pending_response_handle(&read_shared)
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(data);
                return -1;
            }
            data.len() as i32
        },
    )?;

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn dispatch(
    call: HostCall,
    shared: &Arc<Mutex<HostState>>,
    event_sink: &EventSink,
    runtime_state: &Arc<Mutex<SidecarRuntimeState>>,
    request_counter: &Arc<AtomicU64>,
    network: &Arc<NetworkSession>,
    browser: &SharedBrowserSession,
    ai: &SharedAiSession,
) -> crate::host::host_call::HostCallResult {
    match call {
        HostCall::Network(action) => {
            network
                .execute(
                    action,
                    shared.clone(),
                    event_sink.clone(),
                    runtime_state.clone(),
                    request_counter.clone(),
                )
                .await
        }
        HostCall::Browser(action) => {
            super::browser::handle(action, shared.clone(), event_sink.clone(), browser.clone())
                .await
        }
        HostCall::Ai(action) => {
            super::ai::handle(action, shared.clone(), event_sink.clone(), ai.clone()).await
        }
    }
}

fn pending_response_len(shared: &Arc<Mutex<HostState>>) -> Option<i32> {
    pending_response_handle(shared)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref()
        .map(|bytes| bytes.len() as i32)
}

fn take_pending_response(shared: &Arc<Mutex<HostState>>) -> Option<Vec<u8>> {
    pending_response_handle(shared)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()
}

fn pending_response_handle(shared: &Arc<Mutex<HostState>>) -> Arc<Mutex<Option<Vec<u8>>>> {
    let host = lock_runtime(shared, "host_state");
    Arc::clone(&host.pending_response)
}
