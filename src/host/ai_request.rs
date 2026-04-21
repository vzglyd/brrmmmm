use serde::{Deserialize, Serialize};

pub const WIRE_VERSION: u32 = 2;

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum AiAction {
    Complete {
        prompt: String,
    },
    Vision {
        prompt: String,
        image_png_b64: String,
    },
}

impl AiAction {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Complete { .. } => "complete",
            Self::Vision { .. } => "vision",
        }
    }

    pub fn prompt_len(&self) -> usize {
        match self {
            Self::Complete { prompt } | Self::Vision { prompt, .. } => prompt.len(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum AiActionResponse {
    Ok {
        wire_version: u32,
        ok: bool,
        text: String,
    },
    Err {
        wire_version: u32,
        ok: bool,
        error: String,
        message: String,
    },
}

impl AiActionResponse {
    pub fn ok(text: String) -> Self {
        Self::Ok {
            wire_version: WIRE_VERSION,
            ok: true,
            text,
        }
    }

    pub fn err(error: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Err {
            wire_version: WIRE_VERSION,
            ok: false,
            error: error.into(),
            message: message.into(),
        }
    }

    pub fn is_ok(&self) -> bool {
        matches!(self, Self::Ok { .. })
    }

    pub fn error_code(&self) -> Option<&str> {
        match self {
            Self::Ok { .. } => None,
            Self::Err { error, .. } => Some(error),
        }
    }
}

pub fn decode_action(bytes: &[u8]) -> anyhow::Result<AiAction> {
    let val: serde_json::Value = serde_json::from_slice(bytes)?;
    let version = val
        .get("wire_version")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    anyhow::ensure!(
        version == WIRE_VERSION,
        "unsupported ai wire_version {version}; expected {WIRE_VERSION}"
    );
    let action: AiAction = serde_json::from_value(val)?;
    Ok(action)
}

pub fn encode_response(response: &AiActionResponse) -> anyhow::Result<Vec<u8>> {
    Ok(serde_json::to_vec(response)?)
}
