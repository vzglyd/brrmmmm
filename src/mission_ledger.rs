use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::abi::{HostDecisionState, MissionOutcome, MissionRuntimeState};
use crate::config::Config;
use crate::error::{BrrmmmmError, BrrmmmmResult};
use crate::identity::ModuleHash;
use crate::persistence::{FileMode, atomic_write};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RepeatFailureGateRecord {
    pub(crate) reason_code: String,
    pub(crate) input_fingerprint: String,
    pub(crate) triggered_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct MissionLedgerRecord {
    pub(crate) schema_version: u8,
    pub(crate) logical_id: String,
    pub(crate) module_hash: String,
    pub(crate) last_outcome: Option<MissionOutcome>,
    #[serde(default)]
    pub(crate) last_host_decision: Option<HostDecisionState>,
    pub(crate) consecutive_failures: u32,
    pub(crate) last_success_at_ms: Option<u64>,
    pub(crate) last_failure_at_ms: Option<u64>,
    pub(crate) cooldown_until_ms: Option<u64>,
    #[serde(default)]
    pub(crate) last_input_fingerprint: Option<String>,
    #[serde(default)]
    pub(crate) same_reason_streak: u32,
    #[serde(default)]
    pub(crate) repeat_failure_gate: Option<RepeatFailureGateRecord>,
    pub(crate) last_explanation: Option<String>,
}

pub(crate) fn load(
    config: &Config,
    logical_id: &str,
    module_hash: ModuleHash,
) -> BrrmmmmResult<Option<MissionLedgerRecord>> {
    let path = ledger_path(config, logical_id, module_hash);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(BrrmmmmError::PersistenceFailure(format!(
                "read mission ledger {}: {error}",
                path.display()
            )));
        }
    };
    let record = serde_json::from_slice::<MissionLedgerRecord>(&bytes).map_err(|error| {
        BrrmmmmError::StateCorruption(format!("decode mission ledger {}: {error}", path.display()))
    })?;
    Ok(Some(record))
}

pub(crate) fn save(
    config: &Config,
    logical_id: &str,
    module_hash: ModuleHash,
    runtime_state: &MissionRuntimeState,
    input_fingerprint: Option<&str>,
    prior_ledger: Option<&MissionLedgerRecord>,
    same_reason_retry_limit: u32,
) -> BrrmmmmResult<()> {
    let path = ledger_path(config, logical_id, module_hash);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            BrrmmmmError::PersistenceFailure(format!(
                "create mission ledger directory {}: {error}",
                parent.display()
            ))
        })?;
    }
    let (last_input_fingerprint, same_reason_streak, repeat_failure_gate) = ledger_retry_state(
        runtime_state,
        input_fingerprint,
        prior_ledger,
        same_reason_retry_limit,
    );
    let record = MissionLedgerRecord {
        schema_version: 1,
        logical_id: logical_id.to_string(),
        module_hash: module_hash.to_string(),
        last_outcome: runtime_state.last_outcome.clone(),
        last_host_decision: runtime_state.last_host_decision.clone(),
        consecutive_failures: runtime_state.consecutive_failures,
        last_success_at_ms: runtime_state.last_success_at_ms,
        last_failure_at_ms: runtime_state.last_failure_at_ms,
        cooldown_until_ms: runtime_state.cooldown_until_ms,
        last_input_fingerprint,
        same_reason_streak,
        repeat_failure_gate,
        last_explanation: runtime_state
            .last_outcome
            .as_ref()
            .map(|outcome| outcome.message.clone()),
    };
    let bytes = serde_json::to_vec_pretty(&record).map_err(|error| {
        BrrmmmmError::PersistenceFailure(format!("serialize mission ledger: {error}"))
    })?;
    atomic_write(&path, &bytes, FileMode::Private)
}

pub(crate) fn apply_to_runtime_state(
    runtime_state: &mut MissionRuntimeState,
    ledger: &MissionLedgerRecord,
) {
    runtime_state.consecutive_failures = ledger.consecutive_failures;
    runtime_state.last_success_at_ms = ledger.last_success_at_ms;
    runtime_state.last_failure_at_ms = ledger.last_failure_at_ms;
    runtime_state.cooldown_until_ms = ledger.cooldown_until_ms;
}

pub(crate) fn repeat_failure_gate_active(
    ledger: &MissionLedgerRecord,
    input_fingerprint: &str,
) -> bool {
    ledger
        .repeat_failure_gate
        .as_ref()
        .is_some_and(|gate| gate.input_fingerprint == input_fingerprint)
}

fn ledger_retry_state(
    runtime_state: &MissionRuntimeState,
    input_fingerprint: Option<&str>,
    prior_ledger: Option<&MissionLedgerRecord>,
    same_reason_retry_limit: u32,
) -> (Option<String>, u32, Option<RepeatFailureGateRecord>) {
    let fingerprint = input_fingerprint.map(ToOwned::to_owned);
    let Some(outcome) = runtime_state.last_outcome.as_ref() else {
        return (
            fingerprint
                .or_else(|| prior_ledger.and_then(|ledger| ledger.last_input_fingerprint.clone())),
            prior_ledger
                .map(|ledger| ledger.same_reason_streak)
                .unwrap_or(0),
            prior_ledger.and_then(|ledger| ledger.repeat_failure_gate.clone()),
        );
    };

    if outcome.status == crate::abi::MissionOutcomeStatus::Published {
        return (fingerprint, 0, None);
    }

    if outcome.reason_code == "changed_conditions_required" {
        let gate = prior_ledger
            .and_then(|ledger| ledger.repeat_failure_gate.clone())
            .or_else(|| {
                fingerprint
                    .as_ref()
                    .map(|fingerprint| RepeatFailureGateRecord {
                        reason_code: prior_ledger
                            .and_then(|ledger| ledger.last_outcome.as_ref())
                            .map(|outcome| outcome.reason_code.clone())
                            .unwrap_or_else(|| "repeated_failure".to_string()),
                        input_fingerprint: fingerprint.clone(),
                        triggered_at_ms: runtime_state.last_outcome_at_ms.unwrap_or_default(),
                    })
            });
        return (
            fingerprint,
            prior_ledger
                .map(|ledger| ledger.same_reason_streak)
                .unwrap_or(0),
            gate,
        );
    }

    let previous_reason = prior_ledger
        .and_then(|ledger| ledger.last_outcome.as_ref())
        .map(|outcome| outcome.reason_code.as_str());
    let previous_fingerprint =
        prior_ledger.and_then(|ledger| ledger.last_input_fingerprint.as_deref());
    let streak = if previous_reason == Some(outcome.reason_code.as_str())
        && previous_fingerprint == input_fingerprint
    {
        prior_ledger
            .map(|ledger| ledger.same_reason_streak)
            .unwrap_or(0)
            .saturating_add(1)
    } else {
        1
    };
    let gate = if streak >= same_reason_retry_limit {
        fingerprint
            .as_ref()
            .map(|fingerprint| RepeatFailureGateRecord {
                reason_code: outcome.reason_code.clone(),
                input_fingerprint: fingerprint.clone(),
                triggered_at_ms: runtime_state.last_outcome_at_ms.unwrap_or_default(),
            })
    } else {
        None
    };
    (fingerprint, streak, gate)
}

fn ledger_path(config: &Config, logical_id: &str, module_hash: ModuleHash) -> PathBuf {
    config.state_dir.join("mission-ledger").join(format!(
        "{}.{}.json",
        sanitize(logical_id),
        module_hash
    ))
}

fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}
