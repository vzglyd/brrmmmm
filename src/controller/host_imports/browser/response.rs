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
        "browser_response_len",
        move |_caller: wasmtime::Caller<'_, wasmtime_wasi::preview1::WasiP1Ctx>| -> i32 {
            pending_response_len(&len_shared)
        },
    )?;

    linker.func_wrap(
        "vzglyd_host",
        "browser_response_read",
        move |mut caller: wasmtime::Caller<'_, wasmtime_wasi::preview1::WasiP1Ctx>,
              ptr: i32,
              len: i32|
              -> i32 {
            let Some(data) = take_pending_response(&shared) else {
                return -1;
            };
            let write_len = std::cmp::min(data.len(), len as usize);
            if let Err(e) = write_memory_from_caller(&mut caller, ptr, &data[..write_len]) {
                eprintln!("[brrmmmm] browser_response_read error: {e}");
                return -1;
            }
            write_len as i32
        },
    )?;

    Ok(())
}
