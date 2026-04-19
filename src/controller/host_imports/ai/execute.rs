use crate::host::ai_request::{AiAction, AiActionResponse};

const DEFAULT_ANTHROPIC_MODEL: &str = "claude-haiku-4-5-20251001";

pub(super) struct AiSession {
    client: reqwest::blocking::Client,
    model: String,
    api_key: String,
}

impl AiSession {
    pub fn new() -> Self {
        let api_key = std::env::var("ANTHROPIC_API_KEY").unwrap_or_default();
        let model = std::env::var("BRRMMMM_AI_MODEL")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_ANTHROPIC_MODEL.to_string());
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .expect("build reqwest client");
        Self {
            client,
            model,
            api_key,
        }
    }

    pub fn execute(&self, action: AiAction) -> AiActionResponse {
        if self.api_key.is_empty() {
            return AiActionResponse::err(
                "no_api_key",
                "ANTHROPIC_API_KEY is not set on the brrmmmm process",
            );
        }

        let body = match build_request_body(&self.model, &action) {
            Ok(b) => b,
            Err(e) => return AiActionResponse::err("request_build_failed", e.to_string()),
        };

        let resp = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .body(body)
            .send();

        let resp = match resp {
            Ok(r) => r,
            Err(e) => return AiActionResponse::err("request_failed", e.to_string()),
        };

        let status = resp.status();
        let text = match resp.text() {
            Ok(t) => t,
            Err(e) => return AiActionResponse::err("response_read_failed", e.to_string()),
        };

        if !status.is_success() {
            return AiActionResponse::err("api_error", format!("HTTP {status}: {text}"));
        }

        parse_response_text(&text)
    }
}

fn build_request_body(model: &str, action: &AiAction) -> anyhow::Result<Vec<u8>> {
    let content = match action {
        AiAction::Complete { prompt } => {
            serde_json::json!([{"type": "text", "text": prompt}])
        }
        AiAction::Vision {
            prompt,
            image_png_b64,
        } => {
            serde_json::json!([
                {
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": "image/png",
                        "data": image_png_b64
                    }
                },
                {"type": "text", "text": prompt}
            ])
        }
    };

    let body = serde_json::json!({
        "model": model,
        "max_tokens": 1024,
        "messages": [{"role": "user", "content": content}]
    });

    Ok(serde_json::to_vec(&body)?)
}

fn parse_response_text(text: &str) -> AiActionResponse {
    let val: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => return AiActionResponse::err("response_parse_failed", e.to_string()),
    };

    val["content"]
        .as_array()
        .and_then(|blocks| {
            blocks
                .iter()
                .filter(|block| block["type"].as_str().unwrap_or("text") == "text")
                .find_map(|block| block["text"].as_str())
        })
        .map(|t| AiActionResponse::ok(t.to_string()))
        .unwrap_or_else(|| {
            AiActionResponse::err(
                "unexpected_response",
                format!("no text in response: {text}"),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_body_uses_configured_model_for_text_prompts() {
        let body = build_request_body(
            "claude-test-model",
            &AiAction::Complete {
                prompt: "hello".to_string(),
            },
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(value["model"], "claude-test-model");
        assert_eq!(value["messages"][0]["content"][0]["type"], "text");
        assert_eq!(value["messages"][0]["content"][0]["text"], "hello");
    }

    #[test]
    fn request_body_encodes_png_vision_input() {
        let body = build_request_body(
            "claude-test-model",
            &AiAction::Vision {
                prompt: "read this".to_string(),
                image_png_b64: "ZmFrZQ==".to_string(),
            },
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(value["messages"][0]["content"][0]["type"], "image");
        assert_eq!(
            value["messages"][0]["content"][0]["source"]["media_type"],
            "image/png"
        );
        assert_eq!(value["messages"][0]["content"][1]["text"], "read this");
    }

    #[test]
    fn response_parser_returns_first_text_block() {
        let response = parse_response_text(
            r#"{"content":[{"type":"thinking","text":"skip"},{"type":"text","text":"answer"}]}"#,
        );

        match response {
            AiActionResponse::Ok { text, .. } => assert_eq!(text, "answer"),
            other => panic!("expected ok response, got {other:?}"),
        }
    }
}
