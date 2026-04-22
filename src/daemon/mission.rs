use std::collections::VecDeque;
use std::sync::Arc;

use tokio::sync::{Mutex, broadcast, mpsc};

use super::protocol::{MissionSchedulerState, MissionSummary};

pub(super) const MAX_MISSIONS: usize = 128;
pub(super) const BROADCAST_CAPACITY: usize = 1024;
const MAX_HISTORY_EVENTS: usize = 4_096;
const MAX_HISTORY_BYTES: usize = 2 * 1024 * 1024;

pub(super) enum MissionCtrl {
    Hold,
    Resume,
    Abort,
    Retry,
}

pub(super) struct MissionState {
    pub name: String,
    pub wasm: String,
    pub state: MissionSchedulerState,
    pub held: bool,
    pub terminal: bool,
    pub phase: String,
    pub cycles: u64,
    pub pid: Option<u32>,
    pub last_started_at_ms: Option<u64>,
    pub last_run_at_ms: Option<u64>,
    pub last_outcome_status: Option<String>,
    pub next_wake_at_ms: Option<u64>,
    pub last_error: Option<String>,
}

impl MissionState {
    pub fn summary(&self) -> MissionSummary {
        MissionSummary {
            name: self.name.clone(),
            state: self.state,
            phase: self.phase.clone(),
            cycles: self.cycles,
            wasm: self.wasm.clone(),
            held: self.held,
            terminal: self.terminal,
            pid: self.pid,
            last_started_at_ms: self.last_started_at_ms,
            last_run_at_ms: self.last_run_at_ms,
            last_outcome_status: self.last_outcome_status.clone(),
            next_wake_at_ms: self.next_wake_at_ms,
            last_error: self.last_error.clone(),
        }
    }
}

pub(super) struct MissionHandle {
    pub state: Arc<Mutex<MissionState>>,
    pub history: Arc<Mutex<MissionHistory>>,
    pub event_tx: broadcast::Sender<MissionEvent>,
    pub ctrl_tx: mpsc::Sender<MissionCtrl>,
}

#[derive(Debug, Clone)]
pub(super) struct MissionEvent {
    pub seq: u64,
    pub line: String,
}

#[derive(Debug, Default)]
pub(super) struct MissionHistory {
    events: VecDeque<MissionEvent>,
    bytes: usize,
    next_seq: u64,
}

impl MissionHistory {
    pub fn append(&mut self, line: String) -> MissionEvent {
        let event = MissionEvent {
            seq: self.next_seq,
            line,
        };
        self.next_seq = self.next_seq.saturating_add(1);
        self.bytes = self.bytes.saturating_add(event.line.len());
        self.events.push_back(event.clone());

        while self.events.len() > MAX_HISTORY_EVENTS || self.bytes > MAX_HISTORY_BYTES {
            if let Some(removed) = self.events.pop_front() {
                self.bytes = self.bytes.saturating_sub(removed.line.len());
            } else {
                break;
            }
        }

        event
    }

    pub fn snapshot(&self) -> (Vec<MissionEvent>, u64) {
        (self.events.iter().cloned().collect(), self.next_seq)
    }
}

pub(super) struct MissionCleanup {
    name: String,
    tx: mpsc::UnboundedSender<String>,
}

impl MissionCleanup {
    pub const fn new(name: String, tx: mpsc::UnboundedSender<String>) -> Self {
        Self { name, tx }
    }
}

impl Drop for MissionCleanup {
    fn drop(&mut self) {
        let _ = self.tx.send(self.name.clone());
    }
}
