//! Mission-module contract and runtime snapshot types shared between mission
//! modules, the CLI, and the TUI.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// The mission-module ABI version supported by this release of `brrmmmm`.
pub const ABI_VERSION_V3: u32 = 3;

// ── Mission lifecycle phase ──────────────────────────────────────────

/// High-level lifecycle phase reported by a running mission module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MissionPhase {
    /// The mission module is idle and ready to begin work.
    #[default]
    Idle,
    /// The mission module is waiting for a cooldown or retry window to expire.
    CoolingDown,
    /// The mission module is performing acquisition work against a remote or local source.
    Fetching,
    /// The mission module is transforming or validating acquired data.
    Parsing,
    /// The mission module is publishing its final artifact.
    Publishing,
    /// The mission module has reached a terminal failure state for the current mission.
    Failed,
}

// ── Persistence / cooldown authority ────────────────────────────────

/// How durable a mission module's cooldown/rate-limit state is.
///
/// - `volatile`: lives only in RAM; a restart resets it (cooperative only)
/// - `host_persisted`: survives restarts via host-managed storage (solves continuity, not abuse)
/// - `vendor_backed`: enforced by a server-issued lease token (restart cannot bypass it)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PersistenceAuthority {
    /// Cooldown state lives only in memory and is reset by restarting the runtime.
    #[default]
    Volatile,
    /// Cooldown state is stored by the host runtime and survives restarts.
    HostPersisted,
    /// Cooldown state is enforced by a server-issued lease or token outside the runtime.
    VendorBacked,
}

// ── Polling strategy ─────────────────────────────────────────────────

/// Strategy a managed-polling mission module asks the runtime to follow between runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PollStrategy {
    /// Poll at a fixed cadence.
    FixedInterval {
        /// Delay between polls, in seconds.
        interval_secs: u32,
    },
    /// Increase the delay after consecutive failures until `max_secs`.
    ExponentialBackoff {
        /// Initial delay, in seconds, before applying backoff growth.
        base_secs: u32,
        /// Maximum delay, in seconds, reached by the backoff schedule.
        max_secs: u32,
    },
    /// Apply bounded jitter to a base delay to avoid synchronized retries.
    Jittered {
        /// Base delay, in seconds, before jitter is applied.
        base_secs: u32,
        /// Maximum random jitter, in seconds, added to the base delay.
        jitter_secs: u32,
    },
}

// ── Cooldown policy ──────────────────────────────────────────────────

/// Minimum interval policy declared by a mission module for repeated acquisitions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CooldownPolicy {
    /// The persistence model that makes the cooldown durable.
    pub authority: PersistenceAuthority,
    /// The minimum delay, in milliseconds, that should be enforced between runs.
    pub min_interval_ms: u64,
}

// ── Env var specification ────────────────────────────────────────────

/// Metadata describing an environment variable expected by the mission module.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvVarSpec {
    /// The environment variable name the caller should provide.
    pub name: String,
    /// Human-readable guidance explaining the variable's purpose.
    pub description: String,
}

// ── Runtime parameter specification ─────────────────────────────────

/// Schema for structured runtime parameters accepted by a mission module.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MissionParamsSchema {
    /// Declared input fields available to the operator or orchestrator.
    #[serde(default)]
    pub fields: Vec<MissionParamField>,
}

/// Definition of a single parameter accepted by a mission module.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissionParamField {
    /// Stable object key used in the JSON params payload.
    pub key: String,
    /// The value type expected for this field.
    #[serde(rename = "type")]
    pub kind: MissionParamType,
    /// Whether callers must provide a value for this field.
    #[serde(default)]
    pub required: bool,
    /// Optional display label for UIs.
    #[serde(default)]
    pub label: Option<String>,
    /// Optional help text describing the field and any constraints.
    #[serde(default)]
    pub help: Option<String>,
    /// Optional default value supplied when the caller omits the field.
    #[serde(default)]
    pub default: Option<serde_json::Value>,
    /// Enumerated allowed options for the field, when applicable.
    #[serde(default)]
    pub options: Vec<MissionParamOption>,
}

/// One selectable value for a parameter field.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissionParamOption {
    /// The JSON value that should be sent when this option is chosen.
    pub value: serde_json::Value,
    /// Optional display label for the option.
    #[serde(default)]
    pub label: Option<String>,
}

/// Supported JSON-oriented parameter types.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MissionParamType {
    /// UTF-8 string input.
    String,
    /// Signed integer input encoded as JSON number.
    Integer,
    /// Floating-point or integer JSON number input.
    Number,
    /// Boolean input.
    Boolean,
    /// Arbitrary JSON value. Host-side parsing of `Json`-typed param values MUST use
    /// bounded input: validate byte length before deserializing to prevent stack/heap
    /// exhaustion from deeply nested or oversized structures.
    Json,
}

// ── Describe blob (sidecar export) ──────────────────────────────────

/// Full self-description emitted by a mission module at startup.
///
/// This is the core of the brrmmmm behavioral contract: OpenAPI describes
/// the endpoint; this describes the behavior.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissionModuleDescribe {
    /// Version of the describe schema emitted by the mission module.
    pub schema_version: u8,
    /// Stable logical identifier for the mission source.
    pub logical_id: String,
    /// Human-readable mission module name.
    pub name: String,
    /// Human-readable description of the acquisition mission.
    pub description: String,
    /// Mission-module ABI version, or `0` when omitted by older producers.
    #[serde(default)]
    pub abi_version: u32,
    /// Supported runtime modes such as `managed_polling` or `interactive`.
    #[serde(default)]
    pub run_modes: Vec<String>,
    /// Declared durability of cooldown and related continuity state.
    #[serde(default)]
    pub state_persistence: PersistenceAuthority,
    /// Environment variables that must be supplied for the mission module to run.
    #[serde(default)]
    pub required_env_vars: Vec<EnvVarSpec>,
    /// Environment variables that are optional but recognized by the mission module.
    #[serde(default)]
    pub optional_env_vars: Vec<EnvVarSpec>,
    /// Structured runtime parameters accepted by the mission module, when any.
    #[serde(default)]
    pub params: Option<MissionParamsSchema>,
    /// Host capability names required by the mission module.
    #[serde(default)]
    pub capabilities_needed: Vec<String>,
    /// Managed-polling strategy requested by the mission module.
    #[serde(default)]
    pub poll_strategy: Option<PollStrategy>,
    /// Minimum interval policy requested by the mission module.
    #[serde(default)]
    pub cooldown_policy: Option<CooldownPolicy>,
    /// Artifact kinds the mission module may publish.
    #[serde(default)]
    pub artifact_types: Vec<String>,
    /// Optional hard timeout, in seconds, for completing one acquisition mission.
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

/// Metadata describing the most recently observed artifact of a given kind.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactMeta {
    /// Artifact kind such as `published_output`.
    pub kind: String,
    /// Size of the artifact payload in bytes.
    pub size_bytes: usize,
    /// Wall-clock receipt time in Unix milliseconds.
    pub received_at_ms: u64,
}

// ── Guest-emitted event (from take_events ring buffer) ───────────────
// Reserved — take_events host import not yet implemented.
/// Event shape reserved for future guest-originated runtime events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuestEvent {
    /// Guest-reported timestamp in Unix milliseconds.
    pub ts_ms: u64,
    /// Mission-module-defined event kind.
    pub kind: String,
    /// Event-specific structured attributes.
    #[serde(default)]
    pub attrs: serde_json::Value,
}

// ── Active run mode ──────────────────────────────────────────────────

/// Runtime mode currently active for a controller instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ActiveMode {
    /// The mission module is running in the managed polling loop.
    #[default]
    ManagedPolling,
    /// The mission module is running in an operator-driven interactive session.
    Interactive,
}

// ── Mission outcome ──────────────────────────────────────────────────

/// Terminal mission outcome reported by a mission module or synthesized by the host.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MissionOutcomeStatus {
    /// The mission published its final artifact successfully.
    Published,
    /// The mission failed in a way that should be retried later.
    RetryableFailure,
    /// The mission failed terminally and should not be retried automatically.
    TerminalFailure,
    /// The mission requires an operator action before it can continue.
    OperatorActionRequired,
}

/// Typed terminal outcome for one acquisition mission.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MissionOutcome {
    /// Terminal outcome class for this mission.
    pub status: MissionOutcomeStatus,
    /// Stable machine-readable reason code.
    pub reason_code: String,
    /// Human-readable explanation of the outcome.
    pub message: String,
    /// Optional host-enforced retry delay for retryable failures.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
    /// Optional operator task required before the mission can continue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operator_action: Option<String>,
    /// Optional primary artifact kind produced by the mission.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_artifact_kind: Option<String>,
}

// ── Host-side runtime state snapshot ────────────────────────────────

/// Canonical runtime state maintained by the host MissionController.
/// Both the CLI and TUI read this — neither parses logs.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MissionRuntimeState {
    /// The currently active runtime mode.
    pub mode: ActiveMode,
    /// The current lifecycle phase.
    pub phase: MissionPhase,
    /// Earliest time, in Unix milliseconds, that a new attempt is allowed.
    pub next_allowed_at_ms: Option<u64>,
    /// Next scheduled poll time, in Unix milliseconds, for managed polling.
    pub next_scheduled_poll_at_ms: Option<u64>,
    /// Timestamp of the most recent successful run, in Unix milliseconds.
    pub last_success_at_ms: Option<u64>,
    /// Timestamp of the most recent failed run, in Unix milliseconds.
    pub last_failure_at_ms: Option<u64>,
    /// Cooldown expiry time, in Unix milliseconds, when one is active.
    pub cooldown_until_ms: Option<u64>,
    /// Number of consecutive failed attempts tracked by the runtime.
    pub consecutive_failures: u32,
    /// Current backoff delay, in milliseconds, when backoff is active.
    pub backoff_ms: Option<u64>,
    /// Metadata for the most recently observed raw source artifact.
    pub last_raw_artifact: Option<ArtifactMeta>,
    /// Metadata for the most recently published output artifact.
    pub last_output_artifact: Option<ArtifactMeta>,
    /// Human-readable description of the last runtime error, if any.
    pub last_error: Option<String>,
    /// Final mission outcome once the current mission reaches a terminal state.
    pub last_outcome: Option<MissionOutcome>,
    /// Timestamp of the most recent terminal mission outcome report.
    pub last_outcome_at_ms: Option<u64>,
    /// Source of the most recent terminal mission outcome report.
    pub last_outcome_reported_by: Option<String>,
    /// Populated once describe() has been called.
    pub describe: Option<MissionModuleDescribe>,
    /// Persistent key-value storage for session state.
    #[serde(default)]
    pub kv: HashMap<String, Vec<u8>>,
}
