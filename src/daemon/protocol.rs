use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use brrmmmm::abi::MissionModuleDescribe;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    Launch {
        wasm: String,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        env: HashMap<String, String>,
        #[serde(default)]
        params: Option<String>,
    },
    Hold {
        mission: String,
        reason: String,
    },
    Resume {
        mission: String,
    },
    Rescue {
        mission: String,
        action: RescueAction,
        reason: String,
    },
    Abort {
        mission: String,
        reason: String,
    },
    Watch {
        mission: String,
    },
    WatchStatus,
    Status,
    Ping,
    Shutdown,
    Inspect {
        wasm: String,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RescueAction {
    Retry,
    Abort,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Launched { mission: String },
    Ok { mission: String },
    Error { message: String },
    Full { message: String },
    Status { missions: Vec<MissionSummary> },
    Event { mission: String, line: String },
    Pong,
    Bye,
    Inspected {
        describe: Option<MissionModuleDescribe>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MissionSchedulerState {
    Launching,
    Running,
    Scheduled,
    Held,
    AwaitingChange,
    AwaitingOperator,
    TerminalFailure,
    Idle,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MissionSchedulerState {
    Launching,
    Running,
    Scheduled,
    Held,
    AwaitingChange,
    AwaitingOperator,
    TerminalFailure,
    Idle,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MissionSummary {
    pub name: String,
    pub state: MissionSchedulerState,
    pub phase: String,
    pub cycles: u64,
    pub wasm: String,
    pub held: bool,
    pub terminal: bool,
    pub pid: Option<u32>,
    pub last_started_at_ms: Option<u64>,
    pub last_run_at_ms: Option<u64>,
    pub last_outcome_status: Option<String>,
    pub next_wake_at_ms: Option<u64>,
    pub last_error: Option<String>,
}
