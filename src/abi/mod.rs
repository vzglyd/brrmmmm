use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const ABI_VERSION_V1: u32 = 1;

// ── Sidecar lifecycle phase ──────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SidecarPhase {
    #[default]
    Idle,
    CoolingDown,
    Fetching,
    Parsing,
    Publishing,
    Failed,
}

// ── Persistence / cooldown authority ────────────────────────────────

/// How durable a sidecar's cooldown/rate-limit state is.
///
/// - `volatile`: lives only in RAM; a restart resets it (cooperative only)
/// - `host_persisted`: survives restarts via host-managed storage (solves continuity, not abuse)
/// - `vendor_backed`: enforced by a server-issued lease token (restart cannot bypass it)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PersistenceAuthority {
    #[default]
    Volatile,
    HostPersisted,
    VendorBacked,
}

// ── Polling strategy ─────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PollStrategy {
    FixedInterval { interval_secs: u32 },
    ExponentialBackoff { base_secs: u32, max_secs: u32 },
    Jittered { base_secs: u32, jitter_secs: u32 },
}

// ── Cooldown policy ──────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CooldownPolicy {
    pub authority: PersistenceAuthority,
    pub min_interval_ms: u64,
}

// ── Env var specification ────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvVarSpec {
    pub name: String,
    pub description: String,
}

// ── Runtime parameter specification ─────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SidecarParamsSchema {
    #[serde(default)]
    pub fields: Vec<SidecarParamField>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SidecarParamField {
    pub key: String,
    #[serde(rename = "type")]
    pub kind: SidecarParamType,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub help: Option<String>,
    #[serde(default)]
    pub default: Option<serde_json::Value>,
    #[serde(default)]
    pub options: Vec<SidecarParamOption>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SidecarParamOption {
    pub value: serde_json::Value,
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SidecarParamType {
    String,
    Integer,
    Number,
    Boolean,
    /// Arbitrary JSON value. Host-side parsing of `Json`-typed param values MUST use
    /// bounded input: validate byte length before deserializing to prevent stack/heap
    /// exhaustion from deeply nested or oversized structures.
    Json,
}

// ── Describe blob (sidecar export) ──────────────────────────────────

/// Full self-description emitted by a sidecar at startup.
///
/// This is the core of the brrmmmm behavioral contract: OpenAPI describes
/// the endpoint; this describes the behavior.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SidecarDescribe {
    pub schema_version: u8,
    pub logical_id: String,
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub abi_version: u32,
    #[serde(default)]
    pub run_modes: Vec<String>,
    #[serde(default)]
    pub state_persistence: PersistenceAuthority,
    #[serde(default)]
    pub required_env_vars: Vec<EnvVarSpec>,
    #[serde(default)]
    pub optional_env_vars: Vec<EnvVarSpec>,
    #[serde(default)]
    pub params: Option<SidecarParamsSchema>,
    #[serde(default)]
    pub capabilities_needed: Vec<String>,
    #[serde(default)]
    pub poll_strategy: Option<PollStrategy>,
    #[serde(default)]
    pub cooldown_policy: Option<CooldownPolicy>,
    #[serde(default)]
    pub artifact_types: Vec<String>,
    #[serde(default)]
    pub acquisition_timeout_secs: Option<u32>,
}

impl std::fmt::Display for PollStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PollStrategy::FixedInterval { interval_secs } => {
                write!(f, "fixed_interval {interval_secs}s")
            }
            PollStrategy::ExponentialBackoff {
                base_secs,
                max_secs,
            } => {
                write!(f, "exponential_backoff base={base_secs}s max={max_secs}s")
            }
            PollStrategy::Jittered {
                base_secs,
                jitter_secs,
            } => {
                write!(f, "jittered base={base_secs}s jitter={jitter_secs}s")
            }
        }
    }
}

// ── Artifact metadata ────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactMeta {
    pub kind: String,
    pub size_bytes: usize,
    pub received_at_ms: u64,
}

// ── Guest-emitted event (from take_events ring buffer) ───────────────
// Reserved — take_events host import not yet implemented.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuestEvent {
    pub ts_ms: u64,
    pub kind: String,
    #[serde(default)]
    pub attrs: serde_json::Value,
}

// ── Active run mode ──────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ActiveMode {
    #[default]
    ManagedPolling,
    Interactive,
}

// ── Host-side runtime state snapshot ────────────────────────────────

/// Canonical runtime state maintained by the host SidecarController.
/// Both the CLI and TUI read this — neither parses logs.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SidecarRuntimeState {
    pub mode: ActiveMode,
    pub phase: SidecarPhase,
    pub next_allowed_at_ms: Option<u64>,
    pub next_scheduled_poll_at_ms: Option<u64>,
    pub last_success_at_ms: Option<u64>,
    pub last_failure_at_ms: Option<u64>,
    pub cooldown_until_ms: Option<u64>,
    pub consecutive_failures: u32,
    pub backoff_ms: Option<u64>,
    pub last_raw_artifact: Option<ArtifactMeta>,
    pub last_output_artifact: Option<ArtifactMeta>,
    pub last_error: Option<String>,
    /// Populated once describe() has been called.
    pub describe: Option<SidecarDescribe>,
    /// Persistent key-value storage for session state.
    #[serde(default)]
    pub kv: HashMap<String, Vec<u8>>,
}
