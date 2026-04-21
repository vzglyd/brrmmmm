use std::sync::{Arc, Mutex, MutexGuard};

use wasmtime::{Caller, Engine, Linker, Store, StoreLimits, StoreLimitsBuilder};

use crate::abi::{
    ArtifactMeta, DecisionBasisTag, HostDecisionState, MissionModuleDescribe, MissionOutcome,
    MissionOutcomeStatus, MissionPhase, MissionRiskPosture, MissionRuntimeState, NextAttemptPolicy,
    OperatorEscalationState, PollStrategy,
};
use crate::config::{RuntimeAssurance, RuntimeLimits};
use crate::events::{Event, EventSink, diag, now_ms, now_ts};
use crate::host::host_request::ErrorKind;

// ── WASM runtime policy and store types ──────────────────────────────

pub(super) const EPOCH_TICKS_PER_SECOND: u64 = 10;

#[derive(Debug, Clone)]
pub(super) struct RuntimePolicy {
    pub(super) init_timeout_secs: u64,
    pub(super) default_acquisition_timeout_secs: u64,
    pub(super) max_wasm_memory_bytes: usize,
    pub(super) max_table_elements: usize,
    pub(super) max_instances: usize,
    pub(super) max_memories: usize,
    pub(super) max_tables: usize,
    pub(super) max_params_bytes: usize,
    pub(super) max_describe_bytes: usize,
}

impl Default for RuntimePolicy {
    fn default() -> Self {
        Self {
            init_timeout_secs: 60,
            default_acquisition_timeout_secs: 30,
            max_wasm_memory_bytes: 128 * 1024 * 1024,
            max_table_elements: 1_000_000,
            max_instances: 4,
            max_memories: 4,
            max_tables: 8,
            max_params_bytes: 1024 * 1024,
            max_describe_bytes: 1024 * 1024,
        }
    }
}

impl RuntimePolicy {
    pub(super) fn from_limits(limits: &RuntimeLimits) -> Self {
        Self {
            max_params_bytes: limits.max_params_bytes,
            max_describe_bytes: limits.max_host_payload_bytes,
            ..Self::default()
        }
    }

    const fn epoch_ticks_for_secs(secs: u64) -> u64 {
        let ticks = secs.saturating_mul(EPOCH_TICKS_PER_SECOND);
        if ticks == 0 { 1 } else { ticks }
    }

    pub(super) const fn init_deadline_ticks(&self) -> u64 {
        Self::epoch_ticks_for_secs(self.init_timeout_secs)
    }

    pub(super) const fn acquisition_deadline_ticks(timeout_secs: u64) -> u64 {
        Self::epoch_ticks_for_secs(timeout_secs)
    }

    fn store_limits(&self) -> StoreLimits {
        StoreLimitsBuilder::new()
            .memory_size(self.max_wasm_memory_bytes)
            .table_elements(self.max_table_elements)
            .instances(self.max_instances)
            .memories(self.max_memories)
            .tables(self.max_tables)
            .trap_on_grow_failure(true)
            .build()
    }
}

pub(super) struct WasmStoreState {
    pub(super) wasi: wasmtime_wasi::preview1::WasiP1Ctx,
    limits: StoreLimits,
}

impl WasmStoreState {
    fn new(wasi: wasmtime_wasi::preview1::WasiP1Ctx, policy: &RuntimePolicy) -> Self {
        Self {
            wasi,
            limits: policy.store_limits(),
        }
    }
}

pub(super) type WasmCaller<'a> = Caller<'a, WasmStoreState>;
pub(super) type WasmLinker = Linker<WasmStoreState>;
pub(super) type WasmStore = Store<WasmStoreState>;

pub(super) fn build_wasm_store(
    engine: &Engine,
    wasi: wasmtime_wasi::preview1::WasiP1Ctx,
    policy: &RuntimePolicy,
) -> WasmStore {
    let mut store = Store::new(engine, WasmStoreState::new(wasi, policy));
    store.limiter(|state| &mut state.limits);
    store
}

// ── Mutex helpers ────────────────────────────────────────────────────

pub(super) fn lock_runtime<'a, T>(mutex: &'a Mutex<T>, name: &str) -> MutexGuard<'a, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            eprintln!("[brrmmmm] recovering poisoned {name} mutex");
            poisoned.into_inner()
        }
    }
}

// ── Runtime state helpers ────────────────────────────────────────────

pub(super) fn update_phase_state(
    runtime_state: &Arc<Mutex<MissionRuntimeState>>,
    event_sink: &EventSink,
    phase: MissionPhase,
) {
    let previous = {
        let mut state = lock_runtime(runtime_state, "runtime_state");
        let prev = state.phase.clone();
        if !is_valid_phase_transition(&prev, &phase) {
            diag(
                event_sink,
                &format!("[brrmmmm] rejected invalid phase transition {prev:?} -> {phase:?}"),
            );
            return;
        }
        state.phase = phase.clone();
        prev
    };
    tracing::trace!(from = ?previous, to = ?phase, "mission phase transition");
    event_sink.emit(&Event::Phase {
        ts: now_ts(),
        phase,
    });
}

fn is_valid_phase_transition(from: &MissionPhase, to: &MissionPhase) -> bool {
    use MissionPhase::{CoolingDown, Failed, Fetching, Idle, Parsing, Publishing};
    // Self-transitions are always valid (e.g. multiple artifact publishes in sequence).
    if from == to {
        return true;
    }
    // Any phase may transition to Failed or CoolingDown.
    if matches!(to, Failed | CoolingDown) {
        return true;
    }
    matches!(
        (from, to),
        (Idle | CoolingDown | Failed, Fetching)
            | (Idle | Fetching | Parsing, Publishing)
            | (CoolingDown | Failed | Publishing, Idle)
            | (Fetching, Parsing)
    )
}

pub(super) fn update_sleep_state(
    runtime_state: &Arc<Mutex<MissionRuntimeState>>,
    event_sink: &EventSink,
    duration_ms: u64,
    wake_ms: u64,
) {
    {
        let mut state = lock_runtime(runtime_state, "runtime_state");
        if !is_valid_phase_transition(&state.phase, &MissionPhase::CoolingDown) {
            let previous = state.phase.clone();
            drop(state);
            diag(
                event_sink,
                &format!(
                    "[brrmmmm] rejected invalid phase transition {previous:?} -> {:?}",
                    MissionPhase::CoolingDown
                ),
            );
            return;
        }
        state.phase = MissionPhase::CoolingDown;
        state.next_scheduled_poll_at_ms = Some(wake_ms);
        state.cooldown_until_ms = Some(wake_ms);
        state.backoff_ms = Some(duration_ms);
    }
    event_sink.emit(&Event::Phase {
        ts: now_ts(),
        phase: MissionPhase::CoolingDown,
    });
}

pub(super) fn update_artifact_state(
    runtime_state: &Arc<Mutex<MissionRuntimeState>>,
    meta: &ArtifactMeta,
) {
    let mut state = lock_runtime(runtime_state, "runtime_state");
    match meta.kind.as_str() {
        "raw_source_payload" => state.last_raw_artifact = Some(meta.clone()),
        "published_output" => {
            state.last_output_artifact = Some(meta.clone());
            state.last_success_at_ms = Some(meta.received_at_ms);
            state.consecutive_failures = 0;
            state.last_error = None;
        }
        _ => {}
    }
}

pub(super) fn update_failure_state(runtime_state: &Arc<Mutex<MissionRuntimeState>>, error: &str) {
    let mut state = lock_runtime(runtime_state, "runtime_state");
    state.phase = MissionPhase::Failed;
    state.last_failure_at_ms = Some(now_ms());
    state.consecutive_failures = state.consecutive_failures.saturating_add(1);
    state.last_error = Some(error.to_string());
}

pub(super) fn update_mission_outcome_state(
    runtime_state: &Arc<Mutex<MissionRuntimeState>>,
    event_sink: &EventSink,
    outcome: MissionOutcome,
    reported_by: &str,
    assurance: &RuntimeAssurance,
) -> Result<(), String> {
    let now = now_ms();
    let (escalation, host_decision) = {
        let mut state = lock_runtime(runtime_state, "runtime_state");
        let escalation = match outcome.status {
            MissionOutcomeStatus::Published => {
                state.phase = MissionPhase::Publishing;
                state.last_success_at_ms = Some(now);
                state.consecutive_failures = 0;
                state.last_error = None;
                state.next_allowed_at_ms = None;
                state.cooldown_until_ms = None;
                state.backoff_ms = None;
                state.pending_operator_action = None;
                None
            }
            MissionOutcomeStatus::RetryableFailure => {
                state.phase = MissionPhase::Failed;
                state.last_failure_at_ms = Some(now);
                state.consecutive_failures = state.consecutive_failures.saturating_add(1);
                state.last_error = Some(outcome.message.clone());
                state.next_allowed_at_ms = None;
                state.cooldown_until_ms = None;
                state.backoff_ms = None;
                state.pending_operator_action = None;
                if let Some(retry_after_ms) = resolved_retry_after_ms(
                    &outcome,
                    assurance,
                    state.describe.as_ref(),
                    state.consecutive_failures,
                ) {
                    let wake_at = now.saturating_add(retry_after_ms);
                    state.next_allowed_at_ms = Some(wake_at);
                    state.cooldown_until_ms = Some(wake_at);
                    state.backoff_ms = Some(retry_after_ms);
                }
                None
            }
            MissionOutcomeStatus::TerminalFailure => {
                state.phase = MissionPhase::Failed;
                state.last_failure_at_ms = Some(now);
                state.consecutive_failures = state.consecutive_failures.saturating_add(1);
                state.last_error = Some(outcome.message.clone());
                state.next_allowed_at_ms = None;
                state.cooldown_until_ms = None;
                state.backoff_ms = None;
                state.pending_operator_action = None;
                None
            }
            MissionOutcomeStatus::OperatorActionRequired => {
                let escalation =
                    resolve_operator_escalation(state.describe.as_ref(), &outcome, now)?;
                state.phase = MissionPhase::Failed;
                state.last_failure_at_ms = Some(now);
                state.consecutive_failures = state.consecutive_failures.saturating_add(1);
                state.last_error = Some(outcome.message.clone());
                state.next_allowed_at_ms = None;
                state.cooldown_until_ms = None;
                state.backoff_ms = None;
                state.pending_operator_action = Some(escalation.clone());
                Some(escalation)
            }
        };
        let host_decision = host_decision_for_outcome(&outcome, reported_by, escalation.as_ref());
        state.last_outcome = Some(outcome.clone());
        state.last_outcome_at_ms = Some(now);
        state.last_outcome_reported_by = Some(reported_by.to_string());
        state.last_host_decision = Some(host_decision.clone());
        drop(state);
        (escalation, host_decision)
    };
    event_sink.emit(&Event::MissionOutcome {
        ts: now_ts(),
        reported_by: reported_by.to_string(),
        outcome,
        host_decision,
        escalation,
    });
    Ok(())
}

/// Compute a minimum backoff duration from the declared poll strategy and how
/// many consecutive failures have accumulated. This is the floor the runtime
/// enforces regardless of what the module requested via `retry_after_ms`.
pub fn compute_strategy_backoff_ms(strategy: &PollStrategy, consecutive_failures: u32) -> u64 {
    match strategy {
        PollStrategy::FixedInterval { interval_secs } => u64::from(*interval_secs) * 1_000,
        PollStrategy::ExponentialBackoff {
            base_secs,
            max_secs,
        } => {
            // failures=1 → base, failures=2 → 2*base, failures=3 → 4*base, …
            let shift = consecutive_failures.saturating_sub(1);
            let factor = 1u64.checked_shl(shift).unwrap_or(u64::MAX);
            u64::from(*base_secs)
                .saturating_mul(factor)
                .min(u64::from(*max_secs))
                * 1_000
        }
        PollStrategy::Jittered {
            base_secs,
            jitter_secs,
        } => {
            // Deterministic lower bound; the module may add jitter on top.
            u64::from((*base_secs).saturating_sub(*jitter_secs)) * 1_000
        }
    }
}

pub(super) fn resolved_retry_after_ms(
    outcome: &MissionOutcome,
    assurance: &RuntimeAssurance,
    describe: Option<&MissionModuleDescribe>,
    consecutive_failures: u32,
) -> Option<u64> {
    if outcome.status != MissionOutcomeStatus::RetryableFailure {
        return None;
    }
    if outcome.reason_code == "changed_conditions_required" {
        return None;
    }
    if outcome.reason_code == "acquisition_timeout" {
        return outcome
            .retry_after_ms
            .or(Some(assurance.default_retry_after_ms));
    }
    outcome.retry_after_ms.or_else(|| {
        describe
            .and_then(|d| d.poll_strategy.as_ref())
            .map(|s| compute_strategy_backoff_ms(s, consecutive_failures))
            .or(Some(assurance.default_retry_after_ms))
    })
}

pub(super) fn category_for_outcome(outcome: &MissionOutcome) -> &'static str {
    if outcome.reason_code == "acquisition_timeout" {
        return "timeout";
    }
    category_for_status(outcome.status)
}

pub(super) const fn category_for_status(status: MissionOutcomeStatus) -> &'static str {
    match status {
        MissionOutcomeStatus::Published => "published",
        MissionOutcomeStatus::RetryableFailure => "retryable_failure",
        MissionOutcomeStatus::TerminalFailure => "terminal_failure",
        MissionOutcomeStatus::OperatorActionRequired => "operator_action_required",
    }
}

pub(super) fn host_decision_for_outcome(
    outcome: &MissionOutcome,
    reported_by: &str,
    escalation: Option<&OperatorEscalationState>,
) -> HostDecisionState {
    let mut basis = Vec::new();
    let synthesized = reported_by == "host";
    if synthesized {
        basis.push(DecisionBasisTag::HostSynthesized);
    }

    let (risk_posture, next_attempt_policy) = match outcome.status {
        MissionOutcomeStatus::Published => {
            basis.push(DecisionBasisTag::ObjectiveMet);
            (MissionRiskPosture::Nominal, NextAttemptPolicy::None)
        }
        MissionOutcomeStatus::RetryableFailure => {
            basis.push(DecisionBasisTag::ObjectiveNotMet);
            basis.push(DecisionBasisTag::SafeStateEntered);
            if outcome.reason_code == "changed_conditions_required" {
                basis.push(DecisionBasisTag::ChangedConditionsRequired);
                (
                    MissionRiskPosture::AwaitingChangedConditions,
                    NextAttemptPolicy::ManualOnly,
                )
            } else {
                if outcome.retry_after_ms.is_some() {
                    basis.push(DecisionBasisTag::RetryAfterRequested);
                }
                basis.push(DecisionBasisTag::CooldownApplied);
                (
                    MissionRiskPosture::Degraded,
                    NextAttemptPolicy::AfterCooldown,
                )
            }
        }
        MissionOutcomeStatus::TerminalFailure => {
            basis.push(DecisionBasisTag::ObjectiveNotMet);
            basis.push(DecisionBasisTag::SafeStateEntered);
            (
                MissionRiskPosture::ClosedSafe,
                NextAttemptPolicy::ManualOnly,
            )
        }
        MissionOutcomeStatus::OperatorActionRequired => {
            basis.push(DecisionBasisTag::ObjectiveNotMet);
            basis.push(DecisionBasisTag::AutomationExhausted);
            if escalation.is_some() {
                basis.push(DecisionBasisTag::OperatorRescueOpened);
            }
            (
                MissionRiskPosture::AwaitingOperator,
                NextAttemptPolicy::OperatorRescue,
            )
        }
    };

    HostDecisionState {
        category: category_for_outcome(outcome).to_string(),
        synthesized,
        risk_posture,
        next_attempt_policy,
        basis,
    }
}

fn resolve_operator_escalation(
    describe: Option<&MissionModuleDescribe>,
    outcome: &MissionOutcome,
    now_ms: u64,
) -> Result<OperatorEscalationState, String> {
    let policy = describe
        .and_then(|describe| describe.operator_fallback.as_ref())
        .ok_or_else(|| {
            "mission module reported operator_action_required without describe.operator_fallback"
                .to_string()
        })?;
    let timeout_ms = outcome.operator_timeout_ms.unwrap_or(policy.timeout_ms);
    if timeout_ms == 0 {
        return Err(
            "mission module reported operator_action_required with a zero timeout".to_string(),
        );
    }
    let action = outcome
        .operator_action
        .clone()
        .unwrap_or_else(|| "Perform the required operator action before retrying.".to_string());
    let timeout_outcome = outcome
        .operator_timeout_outcome
        .unwrap_or(policy.on_timeout);
    Ok(OperatorEscalationState {
        action,
        deadline_at_ms: now_ms.saturating_add(timeout_ms),
        timeout_outcome,
    })
}

// ── Memory helpers ───────────────────────────────────────────────────

pub(super) fn read_memory_from_caller(
    caller: &mut WasmCaller<'_>,
    ptr: i32,
    len: i32,
) -> anyhow::Result<Vec<u8>> {
    let ptr = checked_memory_offset(ptr, "ptr")?;
    let len = checked_memory_offset(len, "len")?;
    read_memory_range(caller, ptr, len)
}

fn read_memory_range(
    caller: &mut WasmCaller<'_>,
    ptr: usize,
    len: usize,
) -> anyhow::Result<Vec<u8>> {
    let mem = caller
        .get_export("memory")
        .and_then(wasmtime::Extern::into_memory)
        .ok_or_else(|| anyhow::anyhow!("no memory export"))?;
    let data = mem
        .data(caller)
        .get(ptr..)
        .and_then(|s| s.get(..len))
        .ok_or_else(|| anyhow::anyhow!("memory read OOB: ptr={ptr}, len={len}"))?;
    Ok(data.to_vec())
}

pub(super) fn read_limited_memory_from_caller(
    caller: &mut WasmCaller<'_>,
    ptr: i32,
    len: i32,
    limit: usize,
    label: &str,
) -> anyhow::Result<Vec<u8>> {
    let ptr = checked_memory_offset(ptr, "ptr")?;
    let len = checked_memory_offset(len, label)?;
    if len > limit {
        anyhow::bail!("{label} length {len} exceeds configured limit of {limit} bytes");
    }
    read_memory_range(caller, ptr, len)
}

pub(super) fn write_memory_from_caller(
    caller: &mut WasmCaller<'_>,
    ptr: i32,
    data: &[u8],
) -> anyhow::Result<()> {
    let ptr = checked_memory_offset(ptr, "ptr")?;
    let mem = caller
        .get_export("memory")
        .and_then(wasmtime::Extern::into_memory)
        .ok_or_else(|| anyhow::anyhow!("no memory export"))?;
    let mem_data = mem.data_mut(caller);
    let dst = mem_data
        .get_mut(ptr as usize..)
        .and_then(|s| s.get_mut(..data.len()))
        .ok_or_else(|| anyhow::anyhow!("memory write OOB: ptr={ptr}, len={}", data.len()))?;
    dst.copy_from_slice(data);
    Ok(())
}

fn checked_memory_offset(value: i32, label: &str) -> anyhow::Result<usize> {
    usize::try_from(value)
        .map_err(|_| anyhow::anyhow!("memory access invalid negative {label}: {value}"))
}

pub(super) fn classify_reqwest_error(e: &reqwest::Error, message: String) -> (ErrorKind, String) {
    if e.is_timeout() {
        return (ErrorKind::Timeout, message);
    }
    if e.is_connect()
        && let Some(source) = std::error::Error::source(e)
        && let Some(io_err) = source.downcast_ref::<std::io::Error>()
    {
        let kind = io_kind_to_error_kind(io_err.kind());
        if kind != ErrorKind::Io {
            return (kind, message);
        }
    }
    // reqwest surfaces DNS failures as "builder" errors in some configurations;
    // check the message as a heuristic.
    let msg_lower = message.to_ascii_lowercase();
    if msg_lower.contains("dns") || msg_lower.contains("resolve") || msg_lower.contains("lookup") {
        return (ErrorKind::Dns, message);
    }
    if msg_lower.contains("tls") || msg_lower.contains("ssl") || msg_lower.contains("certificate") {
        return (ErrorKind::Tls, message);
    }
    (ErrorKind::Io, message)
}

pub(super) fn classify_io_error(e: &std::io::Error, message: String) -> (ErrorKind, String) {
    (io_kind_to_error_kind(e.kind()), message)
}

const fn io_kind_to_error_kind(k: std::io::ErrorKind) -> ErrorKind {
    match k {
        std::io::ErrorKind::ConnectionRefused => ErrorKind::ConnectionRefused,
        std::io::ErrorKind::PermissionDenied => ErrorKind::PermissionDenied,
        std::io::ErrorKind::TimedOut => ErrorKind::Timeout,
        _ => ErrorKind::Io,
    }
}

#[cfg(test)]
mod tests;
