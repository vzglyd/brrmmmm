mod publish;
mod request;
mod response;
mod state;

use std::sync::{Arc, Mutex, atomic::AtomicU64};

use anyhow::Result;
use wasmtime::Linker;

use crate::abi::SidecarRuntimeState;
use crate::events::EventSink;
use crate::host::HostState;

pub(super) fn register(
    linker: &mut Linker<wasmtime_wasi::preview1::WasiP1Ctx>,
    shared: Arc<Mutex<HostState>>,
    event_sink: EventSink,
    runtime_state: Arc<Mutex<SidecarRuntimeState>>,
    request_counter: Arc<AtomicU64>,
) -> Result<()> {
    request::register(
        linker,
        shared.clone(),
        event_sink,
        runtime_state,
        request_counter,
    )?;
    response::register(linker, shared)?;
    Ok(())
}
