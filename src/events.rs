use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::abi::{ArtifactMeta, SidecarDescribe, SidecarPhase};

// ── Timestamp helpers ────────────────────────────────────────────────

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub fn now_ts() -> String {
    ms_to_iso8601(now_ms())
}

pub fn ms_to_iso8601(ms: u64) -> String {
    let secs = ms / 1000;
    let millis = ms % 1000;
    let (y, mo, d) = civil_from_days((secs / 86400) as i64);
    let h = (secs / 3600) % 24;
    let m = (secs / 60) % 60;
    let s = secs % 60;
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}.{millis:03}Z")
}

/// Howard Hinnant's civil calendar algorithm.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

// ── Event enum ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// Emitted once after WASM is loaded and ABI version is negotiated.
    Started {
        ts: String,
        wasm_path: String,
        wasm_size_bytes: usize,
        abi_version: u32,
    },
    /// Emitted when a v2 sidecar's describe() blob is received.
    Describe {
        ts: String,
        describe: SidecarDescribe,
    },
    /// Emitted once at startup to record which env vars are present.
    EnvSnapshot { ts: String, vars: Vec<EnvVarStatus> },
    /// Emitted when the sidecar's phase changes.
    Phase { ts: String, phase: SidecarPhase },
    /// Forwarded from a v2 sidecar's take_events() ring buffer.
    #[allow(dead_code)]
    GuestEventFwd {
        ts: String,
        guest_ts_ms: u64,
        kind: String,
        attrs: serde_json::Value,
    },
    /// Emitted when any artifact_publish (or channel_push alias) is received.
    ArtifactReceived {
        ts: String,
        kind: String,
        size_bytes: usize,
        /// First 500 bytes of the artifact as a UTF-8 preview.
        preview: String,
        artifact: ArtifactMeta,
    },
    /// Emitted when a network_request starts (before any I/O).
    RequestStart {
        ts: String,
        request_id: String,
        kind: String,
        host: String,
        path: Option<String>,
    },
    /// Emitted when a network_request completes successfully.
    RequestDone {
        ts: String,
        request_id: String,
        status_code: Option<u16>,
        elapsed_ms: u64,
        response_size_bytes: usize,
    },
    /// Emitted when a network_request fails.
    RequestError {
        ts: String,
        request_id: String,
        error_kind: String,
        message: String,
    },
    /// Emitted when the sidecar announces it is about to sleep.
    SleepStart {
        ts: String,
        duration_ms: i64,
        wake_at: String,
    },
    /// Emitted when the sidecar produces a log_info message.
    Log { ts: String, message: String },
    /// Emitted when the sidecar's WASM execution terminates.
    SidecarExit { ts: String, reason: String },
}

// ── Env var status ───────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct EnvVarStatus {
    pub name: String,
    pub required: bool,
    pub set: bool,
}

impl EnvVarStatus {
    /// Build a snapshot from raw `--env KEY=VALUE` args (v1 mode: all provided vars are "set",
    /// required/optional classification is unknown).
    pub fn from_raw_env(env_vars: &[(String, String)]) -> Vec<Self> {
        env_vars
            .iter()
            .map(|(k, _)| Self {
                name: k.clone(),
                required: false, // unknown in v1
                set: true,
            })
            .collect()
    }
}

// ── EventSink ────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct EventSink {
    inner: Arc<Mutex<EventSinkInner>>,
}

struct EventSinkInner {
    enabled: bool,
}

impl EventSink {
    /// A sink that discards all events (normal / --once mode).
    pub fn noop() -> Self {
        Self {
            inner: Arc::new(Mutex::new(EventSinkInner { enabled: false })),
        }
    }

    /// A sink that writes NDJSON to stdout (--events mode).
    pub fn for_stdout() -> Self {
        Self {
            inner: Arc::new(Mutex::new(EventSinkInner { enabled: true })),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.inner.lock().unwrap().enabled
    }

    pub fn emit(&self, event: Event) {
        if !self.inner.lock().unwrap().enabled {
            return;
        }
        if let Ok(json) = serde_json::to_string(&event) {
            let stdout = std::io::stdout();
            let mut handle = stdout.lock();
            let _ = handle.write_all(json.as_bytes());
            let _ = handle.write_all(b"\n");
            let _ = handle.flush();
        }
    }
}

// ── Diagnostic helpers ───────────────────────────────────────────────

/// In non-events mode, print to stderr. In events mode, emit a Log event.
/// This prevents diagnostic messages from corrupting the NDJSON stream.
pub fn diag(sink: &EventSink, msg: &str) {
    if sink.is_enabled() {
        sink.emit(Event::Log {
            ts: now_ts(),
            message: msg.to_string(),
        });
    } else {
        eprintln!("{msg}");
    }
}
