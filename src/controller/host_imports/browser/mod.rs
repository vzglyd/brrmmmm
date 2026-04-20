mod action;
mod execute;
mod response;
mod state;

use std::sync::{Arc, Mutex};

use anyhow::Result;

use crate::events::EventSink;
use crate::host::HostState;

use super::super::io::WasmLinker;
use execute::BrowserSession;

pub(super) fn register(
    linker: &mut WasmLinker,
    shared: Arc<Mutex<HostState>>,
    event_sink: EventSink,
) -> Result<()> {
    let session = BrowserSession::new(shared.clone())?;
    let session = Arc::new(Mutex::new(session));

    action::register(linker, shared.clone(), event_sink, session)?;
    response::register(linker, shared)?;
    Ok(())
}
