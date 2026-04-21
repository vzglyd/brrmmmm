use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::ai_request::AiAction;
use super::browser_request::BrowserAction;
use super::host_request::{NetworkAction, WIRE_VERSION};

#[derive(Debug)]
pub enum HostCall {
    Network(NetworkAction),
    Browser(BrowserAction),
    Ai(AiAction),
}

impl HostCall {
    pub fn capability(&self) -> &'static str {
        match self {
            Self::Network(_) => "network",
            Self::Browser(_) => "browser",
            Self::Ai(_) => "ai",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostCallError {
    pub kind: String,
    pub message: String,
}

impl HostCallError {
    pub fn new(kind: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            message: message.into(),
        }
    }
}

pub type HostCallResult = Result<Value, HostCallError>;

pub fn decode_call(bytes: &[u8]) -> anyhow::Result<HostCall> {
    let val: Value = serde_json::from_slice(bytes)?;
    let version = val
        .get("wire_version")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    anyhow::ensure!(
        version == WIRE_VERSION,
        "unsupported host_call wire_version {version}; expected {WIRE_VERSION}"
    );

    let capability = val
        .get("capability")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("host_call capability is required"))?;

    match capability {
        "network" => Ok(HostCall::Network(serde_json::from_value(val)?)),
        "browser" => Ok(HostCall::Browser(serde_json::from_value(val)?)),
        "ai" => Ok(HostCall::Ai(serde_json::from_value(val)?)),
        other => anyhow::bail!("unsupported host_call capability '{other}'"),
    }
}

pub fn encode_ok(capability: &str, data: Value) -> anyhow::Result<Vec<u8>> {
    Ok(serde_json::to_vec(&serde_json::json!({
        "wire_version": WIRE_VERSION,
        "ok": true,
        "capability": capability,
        "data": data,
    }))?)
}

pub fn encode_error(
    capability: &str,
    kind: impl Into<String>,
    message: impl Into<String>,
) -> anyhow::Result<Vec<u8>> {
    Ok(serde_json::to_vec(&serde_json::json!({
        "wire_version": WIRE_VERSION,
        "ok": false,
        "capability": capability,
        "error": {
            "kind": kind.into(),
            "message": message.into(),
        },
    }))?)
}

pub fn encode_result(capability: &str, result: HostCallResult) -> anyhow::Result<Vec<u8>> {
    match result {
        Ok(data) => encode_ok(capability, data),
        Err(error) => encode_error(capability, error.kind, error.message),
    }
}
