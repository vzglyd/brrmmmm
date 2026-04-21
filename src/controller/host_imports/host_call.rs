use std::future::Future;
use std::sync::{Arc, Mutex, atomic::AtomicU64};

use anyhow::Result;

use crate::abi::MissionRuntimeState;
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
    runtime_state: Arc<Mutex<MissionRuntimeState>>,
    request_counter: Arc<AtomicU64>,
    network: Arc<NetworkSession>,
    browser: SharedBrowserSession,
    ai: SharedAiSession,
) -> Result<()> {
    register_host_call(
        linker,
        shared.clone(),
        event_sink,
        runtime_state,
        request_counter,
        network,
        browser,
        ai,
    )?;
    register_host_response_len(linker, shared.clone())?;
    register_host_response_read(linker, shared)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn register_host_call(
    linker: &mut WasmLinker,
    shared: Arc<Mutex<HostState>>,
    event_sink: EventSink,
    runtime_state: Arc<Mutex<MissionRuntimeState>>,
    request_counter: Arc<AtomicU64>,
    network: Arc<NetworkSession>,
    browser: SharedBrowserSession,
    ai: SharedAiSession,
) -> Result<()> {
    let call_shared = shared;
    let call_sink = event_sink;
    let call_runtime = runtime_state;
    let call_counter = request_counter;
    let call_network = network;
    let call_browser = browser;
    let call_ai = ai;
    linker.func_wrap_async(
        "brrmmmm_host",
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
                let capability = decoded.as_ref().map_or("host", |call| call.capability());
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
                            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(data);
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
    Ok(())
}

fn register_host_response_len(
    linker: &mut WasmLinker,
    shared: Arc<Mutex<HostState>>,
) -> Result<()> {
    linker.func_wrap(
        "brrmmmm_host",
        "host_response_len",
        move |_caller: WasmCaller<'_>| -> i32 { pending_response_len(&shared).unwrap_or(0) },
    )?;
    Ok(())
}

fn register_host_response_read(
    linker: &mut WasmLinker,
    shared: Arc<Mutex<HostState>>,
) -> Result<()> {
    linker.func_wrap(
        "brrmmmm_host",
        "host_response_read",
        move |mut caller: WasmCaller<'_>, ptr: i32, len: i32| -> i32 {
            let Some(data_len) = pending_response_len(&shared) else {
                return -1;
            };
            if len != data_len {
                return -1;
            }
            let Some(data) = take_pending_response(&shared) else {
                return -1;
            };
            if let Err(error) = write_memory_from_caller(&mut caller, ptr, &data) {
                eprintln!("[brrmmmm] host_response_read error: {error}");
                *pending_response_handle(&shared)
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(data);
                return -1;
            }
            len_to_i32(data.len()).unwrap_or(-1)
        },
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn dispatch(
    call: HostCall,
    shared: &Arc<Mutex<HostState>>,
    event_sink: &EventSink,
    runtime_state: &Arc<Mutex<MissionRuntimeState>>,
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
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .and_then(|bytes| len_to_i32(bytes.len()))
}

fn take_pending_response(shared: &Arc<Mutex<HostState>>) -> Option<Vec<u8>> {
    pending_response_handle(shared)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
}

fn pending_response_handle(shared: &Arc<Mutex<HostState>>) -> Arc<Mutex<Option<Vec<u8>>>> {
    let host = lock_runtime(shared, "host_state");
    Arc::clone(&host.pending_response)
}

fn len_to_i32(len: usize) -> Option<i32> {
    i32::try_from(len).ok()
}
