use std::collections::HashMap;

use serde::{Deserialize, Serialize};

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
    Status,
    Ping,
    Shutdown,
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
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MissionSummary {
    pub name: String,
    pub phase: String,
    pub cycles: u64,
    pub wasm: String,
    pub held: bool,
    pub terminal: bool,
    pub pid: Option<u32>,
}
