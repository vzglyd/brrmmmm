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
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Complete { .. } => "complete",
            Self::Vision { .. } => "vision",
        }
    }

    pub const fn prompt_len(&self) -> usize {
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
    pub const fn ok(text: String) -> Self {
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

    pub const fn is_ok(&self) -> bool {
        matches!(self, Self::Ok { .. })
    }

    pub fn error_code(&self) -> Option<&str> {
        match self {
            Self::Ok { .. } => None,
            Self::Err { error, .. } => Some(error),
        }
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn decode_action(bytes: &[u8]) -> anyhow::Result<AiAction> {
    let val: serde_json::Value = serde_json::from_slice(bytes)?;
    let version = u32::try_from(
        val.get("wire_version")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0),
    )
    .unwrap_or(u32::MAX);
    anyhow::ensure!(
        version == WIRE_VERSION,
        "unsupported ai wire_version {version}; expected {WIRE_VERSION}"
    );
    let action: AiAction = serde_json::from_value(val)?;
    Ok(action)
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn encode_response(response: &AiActionResponse) -> anyhow::Result<Vec<u8>> {
    Ok(serde_json::to_vec(response)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_complete_request() {
        let bytes = br#"{"wire_version":2,"action":"complete","prompt":"summarize"}"#;

        let action = decode_action(bytes).unwrap();

        match action {
            AiAction::Complete { prompt } => assert_eq!(prompt, "summarize"),
            AiAction::Vision { .. } => panic!("expected complete action"),
        }
    }

    #[test]
    fn decode_vision_request() {
        let bytes =
            br#"{"wire_version":2,"action":"vision","prompt":"read it","image_png_b64":"ZmFrZQ=="}"#;

        let action = decode_action(bytes).unwrap();

        match action {
            AiAction::Vision {
                prompt,
                image_png_b64,
            } => {
                assert_eq!(prompt, "read it");
                assert_eq!(image_png_b64, "ZmFrZQ==");
            }
            AiAction::Complete { .. } => panic!("expected vision action"),
        }
    }

    #[test]
    fn decode_rejects_wrong_wire_version() {
        let bytes = br#"{"wire_version":3,"action":"complete","prompt":"summarize"}"#;

        let error = decode_action(bytes).unwrap_err().to_string();

        assert!(error.contains("unsupported ai wire_version 3"));
    }

    #[test]
    fn encode_ok_response_includes_text() {
        let bytes = encode_response(&AiActionResponse::ok("answer".to_string())).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(json["wire_version"], u64::from(WIRE_VERSION));
        assert_eq!(json["ok"], true);
        assert_eq!(json["text"], "answer");
    }

    #[test]
    fn encode_error_response_exposes_error_code() {
        let response = AiActionResponse::err("no_api_key", "ANTHROPIC_API_KEY is not set");
        assert_eq!(response.error_code(), Some("no_api_key"));

        let bytes = encode_response(&response).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(json["ok"], false);
        assert_eq!(json["error"], "no_api_key");
        assert_eq!(json["message"], "ANTHROPIC_API_KEY is not set");
    }
}
