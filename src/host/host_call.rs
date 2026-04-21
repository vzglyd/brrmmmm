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
    pub const fn capability(&self) -> &'static str {
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
    let version = u32::try_from(
        val.get("wire_version")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0),
    )
    .unwrap_or(u32::MAX);
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

pub fn encode_ok(capability: &str, data: &Value) -> anyhow::Result<Vec<u8>> {
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
        Ok(data) => encode_ok(capability, &data),
        Err(error) => encode_error(capability, error.kind, error.message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::host_request::{ErrorKind, Header, NetworkAction, NetworkResponseData};

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

        let bytes = encode_ok("network", &data).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(json["wire_version"], u64::from(WIRE_VERSION));
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

        assert_eq!(json["wire_version"], u64::from(WIRE_VERSION));
        assert_eq!(json["ok"], false);
        assert_eq!(json["capability"], "network");
        assert_eq!(json["error"]["kind"], "timeout");
        assert_eq!(json["error"]["message"], "connection timed out");
    }
}
