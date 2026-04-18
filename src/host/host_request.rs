use serde::{Deserialize, Serialize};

pub const WIRE_VERSION: u8 = 1;

// ── Host-mediated request (sidecar → host) ─────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HostRequest {
    HttpsGet {
        host: String,
        path: String,
        #[serde(default)]
        headers: Vec<Header>,
    },
    TcpConnect {
        host: String,
        port: u16,
        #[serde(default = "default_timeout")]
        timeout_ms: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Header {
    pub name: String,
    pub value: String,
}

fn default_timeout() -> u32 {
    5_000
}

// ── Host response (host → sidecar) ─────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HostResponse {
    Http {
        status_code: u16,
        #[serde(default)]
        headers: Vec<Header>,
        body: Vec<u8>,
    },
    TcpConnect {
        elapsed_ms: u64,
    },
    Error {
        error_kind: ErrorKind,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    Dns,
    Tls,
    Io,
    Timeout,
}

// ── Wire encoding/decoding ─────────────────────────────────────────

#[derive(Debug, Serialize)]
#[allow(dead_code)]
struct VersionedRequest<'a> {
    wire_version: u8,
    #[serde(flatten)]
    payload: &'a HostRequest,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct VersionedResponse {
    wire_version: u8,
    #[serde(flatten)]
    payload: HostResponse,
}

pub fn encode_request(req: &HostRequest) -> Result<Vec<u8>, String> {
    let wrapper = VersionedRequest {
        wire_version: WIRE_VERSION,
        payload: req,
    };
    serde_json::to_vec(&wrapper).map_err(|e| format!("encode request: {e}"))
}

pub fn decode_response(bytes: &[u8]) -> Result<HostResponse, String> {
    let wrapper: VersionedResponse =
        serde_json::from_slice(bytes).map_err(|e| format!("decode response: {e}"))?;
    if wrapper.wire_version != WIRE_VERSION {
        return Err(format!(
            "wire version mismatch: expected {}, got {}",
            WIRE_VERSION, wrapper.wire_version
        ));
    }
    Ok(wrapper.payload)
}
