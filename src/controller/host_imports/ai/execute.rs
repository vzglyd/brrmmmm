use crate::host::ai_request::{AiAction, AiActionResponse};

pub(crate) struct AiSession {
    client: reqwest::Client,
    model: String,
    api_key: String,
    max_response_bytes: usize,
}

impl AiSession {
    pub fn new(config: &crate::config::Config) -> anyhow::Result<Self> {
        let api_key = config.anthropic_api_key.clone().unwrap_or_default();
        let model = config.ai_model.clone();
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()?;
        Ok(Self {
            client,
            model,
            api_key,
            max_response_bytes: config.limits.max_ai_response_bytes,
        })
    }

    pub fn prepare_body(&self, action: &AiAction) -> Result<Vec<u8>, AiActionResponse> {
        if self.api_key.is_empty() {
            return Err(AiActionResponse::err(
                "no_api_key",
                "ANTHROPIC_API_KEY is not set on the brrmmmm process",
            ));
        }

        build_request_body(&self.model, action)
            .map_err(|e| AiActionResponse::err("request_build_failed", e.to_string()))
    }

    pub async fn execute_prepared(
        &self,
        body: Vec<u8>,
        user_agent: String,
        attestation_headers: Vec<(String, String)>,
    ) -> AiActionResponse {
        let mut request = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .header(reqwest::header::USER_AGENT, user_agent);
        for (name, value) in attestation_headers {
            request = request.header(name, value);
        }
        let resp = request.body(body).send().await;

        let resp = match resp {
            Ok(r) => r,
            Err(e) => return AiActionResponse::err("request_failed", e.to_string()),
        };

        let status = resp.status();
        if let Some(content_length) = resp.content_length()
            && content_length > self.max_response_bytes as u64
        {
            return AiActionResponse::err(
                "response_too_large",
                format!(
                    "response body is {content_length} bytes, exceeding configured limit of {} bytes",
                    self.max_response_bytes
                ),
            );
        }

        let body = match read_capped_body(resp, self.max_response_bytes).await {
            Ok(body) => body,
            Err(ReadBodyError::TooLarge) => {
                return AiActionResponse::err(
                    "response_too_large",
                    format!(
                        "response body exceeds configured limit of {} bytes",
                        self.max_response_bytes
                    ),
                );
            }
            Err(ReadBodyError::Reqwest(error)) => {
                return AiActionResponse::err("response_read_failed", error.to_string());
            }
        };
        let text = match String::from_utf8(body) {
            Ok(text) => text,
            Err(error) => return AiActionResponse::err("response_read_failed", error.to_string()),
        };

        if !status.is_success() {
            return AiActionResponse::err("api_error", format!("HTTP {status}: {text}"));
        }

        parse_response_text(&text)
    }
}

enum ReadBodyError {
    TooLarge,
    Reqwest(reqwest::Error),
}

async fn read_capped_body(
    mut response: reqwest::Response,
    max_response_bytes: usize,
) -> Result<Vec<u8>, ReadBodyError> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(ReadBodyError::Reqwest)? {
        if body.len().saturating_add(chunk.len()) > max_response_bytes {
            return Err(ReadBodyError::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
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
