use brrmmmm::host::ai_request::{
    AiAction, AiActionResponse, WIRE_VERSION, decode_action, encode_response,
};

#[test]
fn decode_complete_request() {
    let bytes = br#"{"wire_version":1,"action":"complete","prompt":"summarize"}"#;

    let action = decode_action(bytes).unwrap();

    match action {
        AiAction::Complete { prompt } => assert_eq!(prompt, "summarize"),
        _ => panic!("expected complete action"),
    }
}

#[test]
fn decode_vision_request() {
    let bytes =
        br#"{"wire_version":1,"action":"vision","prompt":"read it","image_png_b64":"ZmFrZQ=="}"#;

    let action = decode_action(bytes).unwrap();

    match action {
        AiAction::Vision {
            prompt,
            image_png_b64,
        } => {
            assert_eq!(prompt, "read it");
            assert_eq!(image_png_b64, "ZmFrZQ==");
        }
        _ => panic!("expected vision action"),
    }
}

#[test]
fn decode_rejects_wrong_wire_version() {
    let bytes = br#"{"wire_version":2,"action":"complete","prompt":"summarize"}"#;

    let error = decode_action(bytes).unwrap_err().to_string();

    assert!(error.contains("unsupported ai wire_version 2"));
}

#[test]
fn encode_ok_response_includes_text() {
    let bytes = encode_response(&AiActionResponse::ok("answer".to_string())).unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(json["wire_version"], WIRE_VERSION as u64);
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
