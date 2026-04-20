mod action;
mod execute;
mod response;
mod state;

use std::sync::{Arc, Mutex};

use anyhow::Result;
use wasmtime::Linker;

use crate::events::EventSink;
use crate::host::HostState;

use execute::BrowserSession;

pub(super) fn register(
    linker: &mut Linker<wasmtime_wasi::preview1::WasiP1Ctx>,
    shared: Arc<Mutex<HostState>>,
    event_sink: EventSink,
) -> Result<()> {
    let session = BrowserSession::new(shared.clone())?;
    let session = Arc::new(Mutex::new(session));

    action::register(linker, shared.clone(), event_sink, session)?;
    response::register(linker, shared)?;
    Ok(())
}
