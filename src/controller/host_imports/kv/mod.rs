mod action;
mod response;
pub(super) mod state;

use std::sync::{Arc, Mutex};

use anyhow::Result;

use crate::abi::SidecarRuntimeState;
use crate::events::EventSink;
use crate::host::HostState;

use super::super::io::WasmLinker;

pub(super) fn register(
    linker: &mut WasmLinker,
    shared: Arc<Mutex<HostState>>,
    event_sink: EventSink,
    runtime_state: Arc<Mutex<SidecarRuntimeState>>,
    wasm_hash: Option<String>,
) -> Result<()> {
    action::register(
        linker,
        shared.clone(),
        event_sink.clone(),
        runtime_state.clone(),
        wasm_hash,
    )?;
    response::register(linker, shared)?;
    Ok(())
}
