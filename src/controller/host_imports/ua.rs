use std::sync::{Arc, Mutex};

use anyhow::Result;
use wasmtime::Linker;

use crate::host::HostState;

use super::super::io::{read_memory_from_caller, write_memory_from_caller};

pub(super) fn register(
    linker: &mut Linker<wasmtime_wasi::preview1::WasiP1Ctx>,
    shared: Arc<Mutex<HostState>>,
) -> Result<()> {
    let shared_len = shared.clone();
    let shared_get = shared.clone();
    let shared_set = shared;

    linker.func_wrap(
        "vzglyd_host",
        "ua_get_len",
        move |_caller: wasmtime::Caller<'_, wasmtime_wasi::preview1::WasiP1Ctx>| -> i32 {
            let s = shared_len.lock().unwrap();
            let ua = s.user_agent.lock().unwrap();
            ua.len() as i32
        },
    )?;

    linker.func_wrap(
        "vzglyd_host",
        "ua_get",
        move |mut caller: wasmtime::Caller<'_, wasmtime_wasi::preview1::WasiP1Ctx>,
              ptr: i32,
              len: i32|
              -> i32 {
            let ua_bytes = {
                let s = shared_get.lock().unwrap();
                s.user_agent.lock().unwrap().as_bytes().to_vec()
            };
            let to_write = &ua_bytes[..ua_bytes.len().min(len as usize)];
            match write_memory_from_caller(&mut caller, ptr, to_write) {
                Ok(()) => to_write.len() as i32,
                Err(_) => -1,
            }
        },
    )?;

    linker.func_wrap(
        "vzglyd_host",
        "ua_set",
        move |mut caller: wasmtime::Caller<'_, wasmtime_wasi::preview1::WasiP1Ctx>,
              ptr: i32,
              len: i32|
              -> i32 {
            let bytes = match read_memory_from_caller(&mut caller, ptr, len) {
                Ok(b) => b,
                Err(_) => return -1,
            };
            match String::from_utf8(bytes) {
                Ok(new_ua) => {
                    let s = shared_set.lock().unwrap();
                    *s.user_agent.lock().unwrap() = new_ua;
                    0
                }
                Err(_) => -1,
            }
        },
    )?;

    Ok(())
}
