use std::sync::Arc;

use tokio::sync::{Mutex, broadcast, mpsc};

use super::protocol::MissionSummary;

pub(super) const MAX_MISSIONS: usize = 128;
pub(super) const BROADCAST_CAPACITY: usize = 1024;

pub(super) enum MissionCtrl {
    Hold,
    Resume,
    Abort,
    Retry,
}

pub(super) struct MissionState {
    pub name: String,
    pub wasm: String,
    pub held: bool,
    pub terminal: bool,
    pub phase: String,
    pub cycles: u64,
    pub pid: Option<u32>,
}

impl MissionState {
    pub fn summary(&self) -> MissionSummary {
        MissionSummary {
            name: self.name.clone(),
            phase: self.phase.clone(),
            cycles: self.cycles,
            wasm: self.wasm.clone(),
            held: self.held,
            terminal: self.terminal,
            pid: self.pid,
        }
    }
}

pub(super) struct MissionHandle {
    pub state: Arc<Mutex<MissionState>>,
    pub event_tx: broadcast::Sender<String>,
    pub ctrl_tx: mpsc::Sender<MissionCtrl>,
}

pub(super) struct MissionCleanup {
    name: String,
    tx: mpsc::UnboundedSender<String>,
}

impl MissionCleanup {
    pub fn new(name: String, tx: mpsc::UnboundedSender<String>) -> Self {
        Self { name, tx }
    }
}

impl Drop for MissionCleanup {
    fn drop(&mut self) {
        let _ = self.tx.send(self.name.clone());
    }
}
