use brrmmmm::host::host_request::{
    ErrorKind, Header, HostRequest, HostResponse, WIRE_VERSION, decode_response, encode_request,
};

#[test]
fn encode_https_get_includes_wire_version_and_kind() {
    let req = HostRequest::HttpsGet {
        host: "example.com".to_string(),
        path: "/api/data".to_string(),
        headers: vec![],
    };
    let bytes = encode_request(&req);
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["wire_version"], WIRE_VERSION as u64);
    assert_eq!(json["kind"], "https_get");
    assert_eq!(json["host"], "example.com");
    assert_eq!(json["path"], "/api/data");
}

#[test]
fn encode_tcp_connect_includes_correct_fields() {
    let req = HostRequest::TcpConnect {
        host: "db.internal".to_string(),
        port: 5432,
        timeout_ms: 3000,
    };
    let bytes = encode_request(&req);
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["wire_version"], WIRE_VERSION as u64);
    assert_eq!(json["kind"], "tcp_connect");
    assert_eq!(json["host"], "db.internal");
    assert_eq!(json["port"], 5432);
}

#[test]
fn decode_http_response_succeeds() {
    let json = serde_json::json!({
        "wire_version": WIRE_VERSION,
        "kind": "http",
        "status_code": 200u16,
        "headers": [{"name": "content-type", "value": "application/json"}],
        "body": [104, 101, 108, 108, 111]
    });
    let bytes = serde_json::to_vec(&json).unwrap();
    let response = decode_response(&bytes).unwrap();
    match response {
        HostResponse::Http {
            status_code,
            headers,
            body,
        } => {
            assert_eq!(status_code, 200);
            assert_eq!(headers.len(), 1);
            assert_eq!(headers[0].name, "content-type");
            assert_eq!(body, b"hello");
        }
        _ => panic!("expected Http response"),
    }
}

#[test]
fn decode_rejects_wrong_wire_version() {
    let json = serde_json::json!({
        "wire_version": WIRE_VERSION as u64 + 1,
        "kind": "http",
        "status_code": 200u16,
        "headers": [],
        "body": []
    });
    let bytes = serde_json::to_vec(&json).unwrap();
    let result = decode_response(&bytes);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("wire version mismatch"));
}

#[test]
fn decode_rejects_malformed_json() {
    let result = decode_response(b"not json at all {{{}");
    assert!(result.is_err());
}

#[test]
fn decode_error_response_preserves_kind_and_message() {
    let json = serde_json::json!({
        "wire_version": WIRE_VERSION,
        "kind": "error",
        "error_kind": "timeout",
        "message": "connection timed out"
    });
    let bytes = serde_json::to_vec(&json).unwrap();
    let response = decode_response(&bytes).unwrap();
    match response {
        HostResponse::Error {
            error_kind,
            message,
        } => {
            assert_eq!(error_kind, ErrorKind::Timeout);
            assert_eq!(message, "connection timed out");
        }
        _ => panic!("expected Error response"),
    }
}

#[test]
fn https_get_headers_survive_encode() {
    let req = HostRequest::HttpsGet {
        host: "api.example.com".to_string(),
        path: "/v1/data".to_string(),
        headers: vec![
            Header {
                name: "Authorization".to_string(),
                value: "Bearer token123".to_string(),
            },
            Header {
                name: "Accept".to_string(),
                value: "application/json".to_string(),
            },
        ],
    };
    let bytes = encode_request(&req);
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let headers = json["headers"].as_array().unwrap();
    assert_eq!(headers.len(), 2);
    assert_eq!(headers[0]["name"], "Authorization");
    assert_eq!(headers[1]["name"], "Accept");
}
