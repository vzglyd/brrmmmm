use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::abi::{MissionOutcome, MissionRuntimeState};
use crate::config::Config;
use crate::error::{BrrmmmmError, BrrmmmmResult};
use crate::identity::ModuleHash;
use crate::persistence::{FileMode, atomic_write};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct MissionLedgerRecord {
    pub(crate) schema_version: u8,
    pub(crate) logical_id: String,
    pub(crate) module_hash: String,
    pub(crate) last_outcome: Option<MissionOutcome>,
    pub(crate) consecutive_failures: u32,
    pub(crate) last_success_at_ms: Option<u64>,
    pub(crate) last_failure_at_ms: Option<u64>,
    pub(crate) cooldown_until_ms: Option<u64>,
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
    let record = MissionLedgerRecord {
        schema_version: 1,
        logical_id: logical_id.to_string(),
        module_hash: module_hash.to_string(),
        last_outcome: runtime_state.last_outcome.clone(),
        consecutive_failures: runtime_state.consecutive_failures,
        last_success_at_ms: runtime_state.last_success_at_ms,
        last_failure_at_ms: runtime_state.last_failure_at_ms,
        cooldown_until_ms: runtime_state.cooldown_until_ms,
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
    runtime_state.last_outcome = ledger.last_outcome.clone();
    runtime_state.consecutive_failures = ledger.consecutive_failures;
    runtime_state.last_success_at_ms = ledger.last_success_at_ms;
    runtime_state.last_failure_at_ms = ledger.last_failure_at_ms;
    runtime_state.cooldown_until_ms = ledger.cooldown_until_ms;
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
