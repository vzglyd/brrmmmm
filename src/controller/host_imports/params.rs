use std::sync::{Arc, Mutex};

use crate::events::{Event, EventSink, diag, now_ts};
use crate::host::HostState;

use super::super::io::{
    WasmCaller, WasmLinker, lock_runtime, read_memory_from_caller, write_memory_from_caller,
};

pub(super) fn register(
    linker: &mut WasmLinker,
    shared: Arc<Mutex<HostState>>,
    event_sink: EventSink,
) -> anyhow::Result<()> {
    let s_params_len = shared.clone();
    linker.func_wrap("brrmmmm_host", "params_len", move || -> i32 {
        let params_bytes = params_handle(&s_params_len);
        let params = lock_runtime(&params_bytes, "params_bytes");
        params.as_ref().map_or(0, |params| len_to_i32(params.len()))
    })?;

    let s_params_read = shared;
    let sink_params_read = event_sink.clone();
    linker.func_wrap(
        "brrmmmm_host",
        "params_read",
        move |mut caller: WasmCaller<'_>, ptr: i32, len: i32| -> i32 {
            let Some(buffer_len) = usize::try_from(len).ok() else {
                return -1;
            };
            let params: Vec<u8> = {
                let params_bytes = params_handle(&s_params_read);
                let params = lock_runtime(&params_bytes, "params_bytes");
                params.as_ref().cloned().unwrap_or_default()
            };
            if params.len() > buffer_len {
                return -2;
            }
            match write_memory_from_caller(&mut caller, ptr, &params) {
                Ok(()) => len_to_i32(params.len()),
                Err(error) => {
                    diag(
                        &sink_params_read,
                        &format!("[brrmmmm] params_read memory error: {error}"),
                    );
                    -1
                }
            }
        },
    )?;

    let sink_log = event_sink;
    linker.func_wrap(
        "brrmmmm_host",
        "log_info",
        move |mut caller: WasmCaller<'_>, ptr: i32, len: i32| -> i32 {
            if let Ok(data) = read_memory_from_caller(&mut caller, ptr, len)
                && let Ok(msg) = std::str::from_utf8(&data)
            {
                if sink_log.is_enabled() {
                    sink_log.emit(&Event::Log {
                        ts: now_ts(),
                        message: msg.to_string(),
                    });
                } else {
                    eprintln!("[mission-module] {msg}");
                }
            }
            0
        },
    )?;

    Ok(())
}

fn params_handle(shared: &Arc<Mutex<HostState>>) -> Arc<Mutex<Option<Vec<u8>>>> {
    let host = lock_runtime(shared, "host_state");
    host.params_bytes.clone()
}

fn len_to_i32(len: usize) -> i32 {
    i32::try_from(len).unwrap_or(i32::MAX)
}
