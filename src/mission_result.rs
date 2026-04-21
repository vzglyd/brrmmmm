use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use brrmmmm::abi::{MissionOutcome, MissionOutcomeStatus};
use brrmmmm::controller::MissionCompletion;
use brrmmmm::error::BrrmmmmError;
use brrmmmm::events::{ms_to_iso8601, now_ms};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug)]
pub(crate) struct MissionRecorder {
    path: PathBuf,
    started_at: String,
    started_at_ms: u64,
    wasm_path: Option<String>,
}

impl MissionRecorder {
    pub(crate) fn new(path: PathBuf, wasm_path: Option<&Path>) -> Self {
        let started_at_ms = now_ms();
        Self {
            path,
            started_at: ms_to_iso8601(started_at_ms),
            started_at_ms,
            wasm_path: wasm_path.map(|path| path.display().to_string()),
        }
    }

    pub(crate) fn write_completion(&self, completion: &MissionCompletion) -> Result<()> {
        let finished_at_ms = now_ms();
        let describe = completion.snapshot.describe.as_ref();
        let synthesized = completion.snapshot.last_outcome_reported_by.as_deref() == Some("host");
        let record = MissionRecord {
            schema_version: 2,
            module: MissionModuleRecord {
                wasm_path: self.wasm_path.clone(),
                logical_id: describe.map(|describe| describe.logical_id.clone()),
                name: describe.map(|describe| describe.name.clone()),
                abi_version: describe
                    .map(|describe| describe.abi_version)
                    .filter(|abi_version| *abi_version != 0),
            },
            outcome: completion.outcome.clone(),
            host_decision: HostDecisionRecord {
                exit_code: exit_code_for_outcome(&completion.outcome),
                category: category_for_outcome(&completion.outcome).to_string(),
                synthesized,
            },
            explanation: ExplanationRecord {
                summary: summary_for_outcome(&completion.outcome),
                message: completion.outcome.message.clone(),
                next_action: next_action_for_outcome(&completion.outcome),
            },
            artifacts: MissionArtifactsRecord {
                raw_source: completion.raw_source.as_deref().map(artifact_record),
                normalized: completion.normalized.as_deref().map(artifact_record),
                published_output: completion.published_output.as_deref().map(artifact_record),
            },
            timing: TimingRecord {
                started_at: self.started_at.clone(),
                finished_at: ms_to_iso8601(finished_at_ms),
                elapsed_ms: finished_at_ms.saturating_sub(self.started_at_ms),
            },
            stats: MissionStatsRecord {
                consecutive_failures: completion.snapshot.consecutive_failures,
                last_success_at_ms: completion.snapshot.last_success_at_ms,
                last_failure_at_ms: completion.snapshot.last_failure_at_ms,
                cooldown_until_ms: completion.snapshot.cooldown_until_ms,
            },
        };
        write_record(&self.path, &record)
    }

    pub(crate) fn write_runtime_error(&self, error: &anyhow::Error) -> Result<()> {
        let finished_at_ms = now_ms();
        let outcome = if error_category(error) == "timeout" {
            MissionOutcome {
                status: MissionOutcomeStatus::RetryableFailure,
                reason_code: "acquisition_timeout".to_string(),
                message: format!("{error:#}"),
                retry_after_ms: None,
                operator_action: None,
                primary_artifact_kind: None,
            }
        } else {
            MissionOutcome {
                status: MissionOutcomeStatus::TerminalFailure,
                reason_code: error_category(error).to_string(),
                message: format!("{error:#}"),
                retry_after_ms: None,
                operator_action: None,
                primary_artifact_kind: None,
            }
        };
        let record = MissionRecord {
            schema_version: 2,
            module: MissionModuleRecord {
                wasm_path: self.wasm_path.clone(),
                logical_id: None,
                name: None,
                abi_version: None,
            },
            outcome: outcome.clone(),
            host_decision: HostDecisionRecord {
                exit_code: error_exit_code(error),
                category: error_category(error).to_string(),
                synthesized: true,
            },
            explanation: ExplanationRecord {
                summary: summary_for_outcome(&outcome),
                message: outcome.message.clone(),
                next_action: next_action_for_outcome(&outcome),
            },
            artifacts: MissionArtifactsRecord::default(),
            timing: TimingRecord {
                started_at: self.started_at.clone(),
                finished_at: ms_to_iso8601(finished_at_ms),
                elapsed_ms: finished_at_ms.saturating_sub(self.started_at_ms),
            },
            stats: MissionStatsRecord::default(),
        };
        write_record(&self.path, &record)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct MissionRecord {
    pub(crate) schema_version: u8,
    pub(crate) module: MissionModuleRecord,
    pub(crate) outcome: MissionOutcome,
    pub(crate) host_decision: HostDecisionRecord,
    pub(crate) explanation: ExplanationRecord,
    #[serde(default)]
    pub(crate) artifacts: MissionArtifactsRecord,
    pub(crate) timing: TimingRecord,
    pub(crate) stats: MissionStatsRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct MissionModuleRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) wasm_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) logical_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) abi_version: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct HostDecisionRecord {
    pub(crate) exit_code: i32,
    pub(crate) category: String,
    pub(crate) synthesized: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ExplanationRecord {
    pub(crate) summary: String,
    pub(crate) message: String,
    pub(crate) next_action: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct MissionArtifactsRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) raw_source: Option<MissionArtifactRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) normalized: Option<MissionArtifactRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) published_output: Option<MissionArtifactRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct MissionArtifactRecord {
    pub(crate) size_bytes: usize,
    pub(crate) base64: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) json: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct TimingRecord {
    pub(crate) started_at: String,
    pub(crate) finished_at: String,
    pub(crate) elapsed_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct MissionStatsRecord {
    pub(crate) consecutive_failures: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) last_success_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) last_failure_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) cooldown_until_ms: Option<u64>,
}

pub(crate) fn load_record(path: &Path) -> Result<MissionRecord> {
    let bytes =
        std::fs::read(path).with_context(|| format!("read mission record {}", path.display()))?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("decode mission record {}", path.display()))
}

fn artifact_record(data: &[u8]) -> MissionArtifactRecord {
    MissionArtifactRecord {
        size_bytes: data.len(),
        base64: STANDARD.encode(data),
        json: serde_json::from_slice::<serde_json::Value>(data).ok(),
        text: std::str::from_utf8(data).ok().map(ToOwned::to_owned),
    }
}

fn summary_for_outcome(outcome: &MissionOutcome) -> String {
    match outcome.status {
        MissionOutcomeStatus::Published => format!(
            "Mission published {}.",
            outcome
                .primary_artifact_kind
                .as_deref()
                .unwrap_or("its final artifact")
        ),
        MissionOutcomeStatus::RetryableFailure => {
            format!(
                "Mission failed with a retryable condition: {}.",
                outcome.reason_code
            )
        }
        MissionOutcomeStatus::TerminalFailure => {
            format!("Mission failed terminally: {}.", outcome.reason_code)
        }
        MissionOutcomeStatus::OperatorActionRequired => {
            format!("Mission needs operator action: {}.", outcome.reason_code)
        }
    }
}

fn next_action_for_outcome(outcome: &MissionOutcome) -> String {
    match outcome.status {
        MissionOutcomeStatus::Published => "Consume the published_output artifact.".to_string(),
        MissionOutcomeStatus::RetryableFailure => match outcome.retry_after_ms {
            Some(retry_after_ms) => format!("Retry after {retry_after_ms} ms."),
            None => "Retry when the orchestration policy allows.".to_string(),
        },
        MissionOutcomeStatus::TerminalFailure => {
            "Do not retry automatically; inspect the mission explanation.".to_string()
        }
        MissionOutcomeStatus::OperatorActionRequired => outcome
            .operator_action
            .clone()
            .unwrap_or_else(|| "Perform the required operator action before retrying.".to_string()),
    }
}

fn category_for_outcome(outcome: &MissionOutcome) -> &'static str {
    if outcome.reason_code == "acquisition_timeout" {
        return "timeout";
    }
    match outcome.status {
        MissionOutcomeStatus::Published => "published",
        MissionOutcomeStatus::RetryableFailure => "retryable_failure",
        MissionOutcomeStatus::TerminalFailure => "terminal_failure",
        MissionOutcomeStatus::OperatorActionRequired => "operator_action_required",
    }
}

fn exit_code_for_outcome(outcome: &MissionOutcome) -> i32 {
    match outcome.status {
        MissionOutcomeStatus::Published => 0,
        MissionOutcomeStatus::RetryableFailure => 75,
        MissionOutcomeStatus::TerminalFailure => 70,
        MissionOutcomeStatus::OperatorActionRequired => 65,
    }
}

fn error_category(error: &anyhow::Error) -> &'static str {
    error
        .downcast_ref::<BrrmmmmError>()
        .map(|error| error.category().as_str())
        .unwrap_or("unexpected")
}

fn error_exit_code(error: &anyhow::Error) -> i32 {
    error
        .downcast_ref::<BrrmmmmError>()
        .map(BrrmmmmError::exit_code)
        .unwrap_or(1)
}

fn write_record(path: &Path, record: &MissionRecord) -> Result<()> {
    let bytes =
        serde_json::to_vec_pretty(record).context("serialize mission result record as JSON")?;
    atomic_write(path, &bytes)
}

fn atomic_write(path: &Path, data: &[u8]) -> Result<()> {
    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    std::fs::create_dir_all(parent).map_err(|error| {
        BrrmmmmError::PersistenceFailure(format!(
            "create mission result directory {}: {error}",
            parent.display()
        ))
    })?;

    let mut tmp_path = None;
    let mut tmp_file = None;
    for attempt in 0..32u32 {
        let candidate = parent.join(format!(
            ".{}.{}.{}.tmp",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("mission-record"),
            std::process::id(),
            attempt
        ));
        match open_temp(&candidate) {
            Ok(file) => {
                tmp_path = Some(candidate);
                tmp_file = Some(file);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(BrrmmmmError::PersistenceFailure(format!(
                    "open temp mission result {}: {error}",
                    candidate.display()
                ))
                .into());
            }
        }
    }

    let tmp_path = tmp_path.ok_or_else(|| {
        BrrmmmmError::PersistenceFailure(format!(
            "allocate temp mission result file next to {} after 32 attempts",
            path.display()
        ))
    })?;
    let mut file = tmp_file.expect("tmp file set with tmp path");

    let result = (|| -> Result<()> {
        file.write_all(data).map_err(|error| {
            BrrmmmmError::PersistenceFailure(format!(
                "write mission result {}: {error}",
                tmp_path.display()
            ))
        })?;
        file.sync_all().map_err(|error| {
            BrrmmmmError::PersistenceFailure(format!(
                "fsync mission result {}: {error}",
                tmp_path.display()
            ))
        })?;
        drop(file);
        std::fs::rename(&tmp_path, path).map_err(|error| {
            BrrmmmmError::PersistenceFailure(format!(
                "rename mission result {} to {}: {error}",
                tmp_path.display(),
                path.display()
            ))
        })?;
        fsync_dir(parent)?;
        Ok(())
    })();

    if result.is_err() {
        let _ = std::fs::remove_file(&tmp_path);
    }
    result
}

#[cfg(unix)]
fn open_temp(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt as _;

    std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn open_temp(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
}

fn fsync_dir(path: &Path) -> Result<()> {
    std::fs::File::open(path)
        .map_err(|error| {
            BrrmmmmError::PersistenceFailure(format!(
                "open mission result directory {}: {error}",
                path.display()
            ))
        })?
        .sync_all()
        .map_err(|error| {
            BrrmmmmError::PersistenceFailure(format!(
                "fsync mission result directory {}: {error}",
                path.display()
            ))
        })?;
    Ok(())
}
