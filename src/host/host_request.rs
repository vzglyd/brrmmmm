use serde::{Deserialize, Serialize};

pub const WIRE_VERSION: u32 = 2;

// ── Host-mediated network request ───────────────────────────────────

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum NetworkAction {
    Http {
        method: String,
        url: String,
        #[serde(default)]
        headers: Vec<Header>,
        #[serde(default)]
        body_base64: Option<String>,
        #[serde(default = "default_http_timeout_ms")]
        timeout_ms: u32,
    },
    TcpConnect {
        host: String,
        port: u16,
        #[serde(default = "default_tcp_timeout_ms")]
        timeout_ms: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Header {
    pub name: String,
    pub value: String,
}

fn default_http_timeout_ms() -> u32 {
    30_000
}

fn default_tcp_timeout_ms() -> u32 {
    5_000
}

// ── Host response data ──────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NetworkResponseData {
    Http {
        status_code: u16,
        #[serde(default)]
        headers: Vec<Header>,
        body_base64: String,
    },
    TcpConnect {
        elapsed_ms: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    Dns,
    Tls,
    Io,
    Timeout,
    ConnectionRefused,
    PermissionDenied,
    Unknown,
}

impl ErrorKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Dns => "dns",
            Self::Tls => "tls",
            Self::Io => "io",
            Self::Timeout => "timeout",
            Self::ConnectionRefused => "connection_refused",
            Self::PermissionDenied => "permission_denied",
            Self::Unknown => "unknown",
        }
    }
}
