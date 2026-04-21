use brrmmmm::host::host_call::{HostCall, decode_call, encode_error, encode_ok};
use brrmmmm::host::host_request::{
    ErrorKind, Header, NetworkAction, NetworkResponseData, WIRE_VERSION,
};

#[test]
fn decode_http_request_accepts_full_url_and_headers() {
    let bytes = br#"{
        "wire_version":2,
        "capability":"network",
        "action":"http",
        "method":"GET",
        "url":"https://example.com/api/data",
        "headers":[{"name":"accept","value":"application/json"}]
    }"#;

    let call = decode_call(bytes).unwrap();

    match call {
        HostCall::Network(NetworkAction::Http {
            method,
            url,
            headers,
            body_base64,
            timeout_ms,
        }) => {
            assert_eq!(method, "GET");
            assert_eq!(url, "https://example.com/api/data");
            assert_eq!(headers.len(), 1);
            assert_eq!(headers[0].name, "accept");
            assert!(body_base64.is_none());
            assert_eq!(timeout_ms, 30_000);
        }
        _ => panic!("expected network http action"),
    }
}

#[test]
fn decode_tcp_connect_request_preserves_timeout() {
    let bytes = br#"{
        "wire_version":2,
        "capability":"network",
        "action":"tcp_connect",
        "host":"db.internal",
        "port":5432,
        "timeout_ms":3000
    }"#;

    let call = decode_call(bytes).unwrap();

    match call {
        HostCall::Network(NetworkAction::TcpConnect {
            host,
            port,
            timeout_ms,
        }) => {
            assert_eq!(host, "db.internal");
            assert_eq!(port, 5432);
            assert_eq!(timeout_ms, 3000);
        }
        _ => panic!("expected tcp_connect action"),
    }
}

#[test]
fn encode_ok_wraps_network_response_data() {
    let response = NetworkResponseData::Http {
        status_code: 200,
        headers: vec![Header {
            name: "content-type".to_string(),
            value: "application/json".to_string(),
        }],
        body_base64: "aGVsbG8=".to_string(),
    };
    let data = serde_json::to_value(response).unwrap();

    let bytes = encode_ok("network", data).unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(json["wire_version"], WIRE_VERSION as u64);
    assert_eq!(json["ok"], true);
    assert_eq!(json["capability"], "network");
    assert_eq!(json["data"]["kind"], "http");
    assert_eq!(json["data"]["status_code"], 200);
    assert_eq!(json["data"]["body_base64"], "aGVsbG8=");
}

#[test]
fn encode_error_wraps_kind_and_message() {
    let bytes = encode_error(
        "network",
        ErrorKind::Timeout.as_str(),
        "connection timed out",
    )
    .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(json["wire_version"], WIRE_VERSION as u64);
    assert_eq!(json["ok"], false);
    assert_eq!(json["capability"], "network");
    assert_eq!(json["error"]["kind"], "timeout");
    assert_eq!(json["error"]["message"], "connection timed out");
}
