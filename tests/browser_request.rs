use brrmmmm::host::browser_request::{
    BrowserAction, BrowserActionResponse, SelectorKind, WIRE_VERSION, decode_action,
    encode_response,
};

#[test]
fn decode_screenshot_action() {
    let bytes = br#"{"wire_version":2,"action":"screenshot"}"#;

    let action = decode_action(bytes).unwrap();

    assert!(matches!(action, BrowserAction::Screenshot));
}

#[test]
fn decode_get_html_accepts_xpath_selector_kind() {
    let bytes = br#"{"wire_version":2,"action":"get_html","selector":"//div[@role='main']","selector_kind":"xpath","limit":3}"#;

    let action = decode_action(bytes).unwrap();

    match action {
        BrowserAction::GetHtml {
            selector,
            selector_kind,
            limit,
        } => {
            assert_eq!(selector.as_deref(), Some("//div[@role='main']"));
            assert!(matches!(selector_kind, SelectorKind::XPath));
            assert_eq!(limit, 3);
        }
        _ => panic!("expected get_html action"),
    }
}

#[test]
fn decode_get_html_defaults_to_document_css_mode() {
    let bytes = br#"{"wire_version":2,"action":"get_html"}"#;

    let action = decode_action(bytes).unwrap();

    match action {
        BrowserAction::GetHtml {
            selector,
            selector_kind,
            limit,
        } => {
            assert!(selector.is_none());
            assert!(matches!(selector_kind, SelectorKind::Css));
            assert_eq!(limit, 20);
        }
        _ => panic!("expected get_html action"),
    }
}

#[test]
fn encode_screenshot_response_includes_png_payload() {
    let bytes = encode_response(&BrowserActionResponse::ok_screenshot(
        "iVBORw0KGgo=".to_string(),
    ))
    .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(json["wire_version"], WIRE_VERSION as u64);
    assert_eq!(json["ok"], true);
    assert_eq!(json["png_b64"], "iVBORw0KGgo=");
}
