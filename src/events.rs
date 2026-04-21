//! Structured runtime events and timestamp helpers.

use std::io::Write;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::abi::{
    ArtifactMeta, HostDecisionState, MissionModuleDescribe, MissionOutcome, MissionPhase,
    OperatorEscalationState,
};

// ── Timestamp helpers ────────────────────────────────────────────────

/// Return the current Unix timestamp in milliseconds.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Return the current UTC time encoded as an ISO-8601 string with millisecond precision.
pub fn now_ts() -> String {
    ms_to_iso8601(now_ms())
}

/// Convert a Unix timestamp in milliseconds to an ISO-8601 UTC string.
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

/// Structured event emitted by the runtime in `--events` mode.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// Emitted once after WASM is loaded and ABI version is negotiated.
    Started {
        /// Event timestamp in ISO-8601 UTC format.
        ts: String,
        /// Path to the loaded WASM module.
        wasm_path: String,
        /// Size of the loaded WASM module in bytes.
        wasm_size_bytes: usize,
        /// ABI version reported by the mission module.
        abi_version: u32,
    },
    /// Emitted when a mission module's describe() blob is received.
    Describe {
        /// Event timestamp in ISO-8601 UTC format.
        ts: String,
        /// Full describe contract emitted by the mission module.
        describe: MissionModuleDescribe,
    },
    /// Emitted once at startup to record which env vars are present.
    EnvSnapshot {
        /// Event timestamp in ISO-8601 UTC format.
        ts: String,
        /// Presence snapshot for CLI-provided environment variables.
        vars: Vec<EnvVarStatus>,
    },
    /// Emitted when the mission module's phase changes.
    Phase {
        /// Event timestamp in ISO-8601 UTC format.
        ts: String,
        /// Newly observed lifecycle phase.
        phase: MissionPhase,
    },
    /// Forwarded from a mission module's take_events() ring buffer.
    #[allow(dead_code)]
    GuestEventFwd {
        /// Event timestamp in ISO-8601 UTC format.
        ts: String,
        /// Guest-provided timestamp in Unix milliseconds.
        guest_ts_ms: u64,
        /// Guest-defined event kind.
        kind: String,
        /// Guest-defined structured attributes.
        attrs: serde_json::Value,
    },
    /// Emitted when an artifact_publish is received.
    ArtifactReceived {
        /// Event timestamp in ISO-8601 UTC format.
        ts: String,
        /// Artifact kind such as `published_output`.
        kind: String,
        /// Size of the artifact payload in bytes.
        size_bytes: usize,
        /// First 500 bytes of the artifact as a UTF-8 preview.
        preview: String,
        /// Structured artifact metadata snapshot.
        artifact: ArtifactMeta,
    },
    /// Emitted when a network_request starts (before any I/O).
    RequestStart {
        /// Event timestamp in ISO-8601 UTC format.
        ts: String,
        /// Runtime-generated request identifier for correlation.
        request_id: String,
        /// Action kind such as `http` or `tcp_connect`.
        kind: String,
        /// Remote host or authority being contacted.
        host: String,
        /// Request path when one is available.
        path: Option<String>,
    },
    /// Emitted when a network_request completes successfully.
    RequestDone {
        /// Event timestamp in ISO-8601 UTC format.
        ts: String,
        /// Runtime-generated request identifier for correlation.
        request_id: String,
        /// HTTP status code when the action returned an HTTP response.
        status_code: Option<u16>,
        /// Total elapsed time in milliseconds.
        elapsed_ms: u64,
        /// Response payload size in bytes.
        response_size_bytes: usize,
    },
    /// Emitted when a network_request fails.
    RequestError {
        /// Event timestamp in ISO-8601 UTC format.
        ts: String,
        /// Runtime-generated request identifier for correlation.
        request_id: String,
        /// Stable runtime error kind.
        error_kind: String,
        /// Human-readable error message.
        message: String,
    },
    /// Emitted when the mission module announces it is about to sleep.
    SleepStart {
        /// Event timestamp in ISO-8601 UTC format.
        ts: String,
        /// Requested sleep duration in milliseconds.
        duration_ms: i64,
        /// Planned wake time in ISO-8601 UTC format.
        wake_at: String,
    },
    /// Emitted when the mission module produces a log_info message.
    Log {
        /// Event timestamp in ISO-8601 UTC format.
        ts: String,
        /// Mission-module or runtime log message.
        message: String,
    },
    /// Emitted when the mission module reports its final outcome.
    MissionOutcome {
        /// Event timestamp in ISO-8601 UTC format.
        ts: String,
        /// Whether the outcome came from the mission module or was synthesized by the host.
        reported_by: String,
        /// Structured terminal mission outcome.
        outcome: MissionOutcome,
        /// Runtime-owned interpretation of the outcome and next-attempt rules.
        host_decision: HostDecisionState,
        /// Resolved bounded operator-rescue window for this attempt, when one exists.
        escalation: Option<OperatorEscalationState>,
    },
    /// Emitted when the mission module's WASM execution terminates.
    ModuleExit {
        /// Event timestamp in ISO-8601 UTC format.
        ts: String,
        /// Human-readable termination reason.
        reason: String,
    },
    /// Emitted when a browser_action starts.
    BrowserAction {
        /// Event timestamp in ISO-8601 UTC format.
        ts: String,
        /// Browser action kind.
        action: String,
        /// Loggable detail (selector or URL); never includes secret values.
        detail: String,
    },
    /// Emitted when a browser_action completes.
    BrowserActionDone {
        /// Event timestamp in ISO-8601 UTC format.
        ts: String,
        /// Browser action kind.
        action: String,
        /// Total elapsed time in milliseconds.
        elapsed_ms: u64,
        /// Whether the browser action succeeded.
        ok: bool,
        /// Error description when the action failed.
        error: Option<String>,
    },
    /// Emitted when an ai_request starts.
    AiRequest {
        /// Event timestamp in ISO-8601 UTC format.
        ts: String,
        /// AI action kind.
        action: String,
        /// Prompt length in bytes.
        prompt_len: usize,
    },
    /// Emitted when an ai_request completes.
    AiRequestDone {
        /// Event timestamp in ISO-8601 UTC format.
        ts: String,
        /// AI action kind.
        action: String,
        /// Total elapsed time in milliseconds.
        elapsed_ms: u64,
        /// Whether the AI request succeeded.
        ok: bool,
        /// Error description when the action failed.
        error: Option<String>,
    },
    /// Emitted when a kv_get is called.
    KvGet {
        /// Event timestamp in ISO-8601 UTC format.
        ts: String,
        /// Requested key.
        key: String,
        /// Whether the key existed.
        found: bool,
    },
    /// Emitted when a kv_set is called.
    KvSet {
        /// Event timestamp in ISO-8601 UTC format.
        ts: String,
        /// Written key.
        key: String,
        /// Value length in bytes.
        value_len: usize,
    },
    /// Emitted when a kv_delete is called.
    KvDelete {
        /// Event timestamp in ISO-8601 UTC format.
        ts: String,
        /// Deleted key.
        key: String,
    },
}

// ── Env var status ───────────────────────────────────────────────────

/// Environment-variable presence summary included in startup events.
#[derive(Debug, Clone, Serialize)]
pub struct EnvVarStatus {
    /// Environment variable name.
    pub name: String,
    /// Whether the variable is known to be required by the mission module.
    pub required: bool,
    /// Whether the variable was provided by the caller.
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

/// Event sink that either discards runtime events or writes them as NDJSON.
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

    /// Return `true` when the sink actively emits structured events.
    pub fn is_enabled(&self) -> bool {
        lock_sink(&self.inner).enabled
    }

    /// Emit one structured event if the sink is enabled.
    pub fn emit(&self, event: Event) {
        if !lock_sink(&self.inner).enabled {
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

fn lock_sink<'a>(inner: &'a Mutex<EventSinkInner>) -> MutexGuard<'a, EventSinkInner> {
    match inner.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            eprintln!("[brrmmmm] recovering poisoned event sink mutex");
            poisoned.into_inner()
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
