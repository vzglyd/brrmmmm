use std::sync::{Arc, Mutex};

use anyhow::Result;
use wasmtime::Linker;

use crate::host::HostState;

use super::super::super::io::write_memory_from_caller;
use super::state::{pending_response_len, take_pending_response};

pub(super) fn register(
    linker: &mut Linker<wasmtime_wasi::preview1::WasiP1Ctx>,
    shared: Arc<Mutex<HostState>>,
) -> Result<()> {
    let len_shared = shared.clone();
    linker.func_wrap(
        "vzglyd_host",
        "network_response_len",
        move |_caller: wasmtime::Caller<'_, wasmtime_wasi::preview1::WasiP1Ctx>| -> i32 {
            pending_response_len(&len_shared)
        },
    )?;

    let peek_shared = shared.clone();
    linker.func_wrap(
        "vzglyd_host",
        "network_response_read",
        move |mut caller: wasmtime::Caller<'_, wasmtime_wasi::preview1::WasiP1Ctx>,
              ptr: i32,
              len: i32|
              -> i32 {
            let data_len = pending_response_len(&peek_shared);
            if data_len < 0 {
                return -1;
            }
            if len < data_len {
                return -1;
            }
            let Some(data) = take_pending_response(&shared) else {
                return -1;
            };
            if let Err(error) = write_memory_from_caller(&mut caller, ptr, &data) {
                eprintln!("[brrmmmm] network_response_read error: {error}");
                return -1;
            }
            data.len() as i32
        },
    )?;

    Ok(())
}
