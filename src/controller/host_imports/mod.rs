mod ai;
mod artifacts;
mod browser;
mod kv;
mod network;
mod params;
mod sleep;
mod tracing;
mod ua;

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64},
};

use anyhow::Result;

use crate::abi::SidecarRuntimeState;
use crate::events::EventSink;
use crate::host::HostState;

use super::io::WasmLinker;

pub(super) fn register_vzglyd_host_on_linker(
    linker: &mut WasmLinker,
    host_state: HostState,
    event_sink: EventSink,
    runtime_state: Arc<Mutex<SidecarRuntimeState>>,
    stop_signal: Arc<AtomicBool>,
    force_refresh: Arc<AtomicBool>,
    wasm_hash: Option<String>,
    config: &crate::config::Config,
) -> Result<Arc<Mutex<HostState>>> {
    let shared = Arc::new(Mutex::new(host_state));
    let request_counter = Arc::new(AtomicU64::new(0));

    artifacts::register(
        linker,
        shared.clone(),
        event_sink.clone(),
        runtime_state.clone(),
    )?;
    params::register(linker, shared.clone(), event_sink.clone())?;
    sleep::register(
        linker,
        event_sink.clone(),
        runtime_state.clone(),
        stop_signal,
        force_refresh,
    )?;
    network::register(
        linker,
        shared.clone(),
        event_sink.clone(),
        runtime_state.clone(),
        request_counter,
    )?;
    browser::register(linker, shared.clone(), event_sink.clone())?;
    ai::register(linker, shared.clone(), event_sink.clone(), config)?;
    kv::register(linker, shared.clone(), event_sink, runtime_state, wasm_hash)?;
    tracing::register(linker)?;
    ua::register(linker, shared.clone())?;

    Ok(shared)
}
