use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use brrmmmm::abi::{
    DecisionBasisTag, EnvVarSpec, HostDecisionState, MissionModuleDescribe, MissionOutcome,
    MissionOutcomeStatus, MissionParamsSchema, MissionRiskPosture, NextAttemptPolicy,
    OperatorEscalationState, OperatorTimeoutOutcome, PersistenceAuthority,
};
use brrmmmm::controller::MissionCompletion;
use brrmmmm::error::BrrmmmmError;
use brrmmmm::events::{ms_to_iso8601, now_ms};
use serde::{Deserialize, Serialize};

const RESULT_SCHEMA_VERSION: u8 = 1;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MissionRecordKind {
    #[default]
    Result,
    Status,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MissionJobMode {
    #[default]
    Once,
    Daemon,
}

#[derive(Clone, Debug)]
pub struct MissionRecorder {
    path: PathBuf,
    started_at: String,
    started_at_ms: u64,
    wasm_path: Option<String>,
    mode: MissionJobMode,
}

impl MissionRecorder {
    pub(crate) fn new(path: PathBuf, wasm_path: Option<&Path>) -> Self {
        Self::new_for_mode(path, wasm_path, MissionJobMode::Once)
    }

    pub(crate) fn for_daemon(path: PathBuf, wasm_path: Option<&Path>) -> Self {
        Self::new_for_mode(path, wasm_path, MissionJobMode::Daemon)
    }

    fn new_for_mode(path: PathBuf, wasm_path: Option<&Path>, mode: MissionJobMode) -> Self {
        let started_at_ms = now_ms();
        Self {
            path,
            started_at: ms_to_iso8601(started_at_ms),
            started_at_ms,
            wasm_path: wasm_path.map(|path| path.display().to_string()),
            mode,
        }
    }

    pub(crate) fn write_completion(&self, completion: &MissionCompletion) -> Result<()> {
        self.write_completion_with_context(completion, MissionRecordContext::default())
    }

    pub(crate) fn write_completion_with_context(
        &self,
        completion: &MissionCompletion,
        context: MissionRecordContext,
    ) -> Result<()> {
        let finished_at_ms = now_ms();
        let describe = completion.snapshot.describe.as_ref();
        let synthesized = completion.snapshot.last_outcome_reported_by.as_deref() == Some("host");
        let escalation = completion
            .snapshot
            .pending_operator_action
            .as_ref()
            .map(escalation_record);
        let host_decision = host_decision_record(
            completion
                .snapshot
                .last_host_decision
                .clone()
                .unwrap_or_else(|| fallback_host_decision(&completion.outcome, synthesized)),
            &completion.outcome,
            true,
        );
        let artifacts = MissionArtifactsRecord {
            raw_source: completion.raw_source.as_deref().map(artifact_record),
            normalized: completion.normalized.as_deref().map(artifact_record),
            published_output: completion.published_output.as_deref().map(artifact_record),
        };
        let record = MissionRecord {
            schema_version: RESULT_SCHEMA_VERSION,
            record_kind: MissionRecordKind::Result,
            job: context.job.or_else(|| Some(self.default_job_record())),
            mission: context
                .mission
                .or_else(|| Some(mission_contract_record(self.wasm_path.as_deref(), describe))),
            attempt: context.attempt.or_else(|| {
                Some(default_attempt_record(
                    1,
                    None,
                    Some(phase_name_from_completion(completion)),
                    self.started_at.clone(),
                    finished_at_ms,
                    true,
                ))
            }),
            timeline: context.timeline,
            challenges: context.challenges,
            interventions: context.interventions,
            payload: completion
                .published_output
                .as_deref()
                .map(|data| payload_record(&completion.outcome, data)),
            module: MissionModuleRecord {
                wasm_path: self.wasm_path.clone(),
                logical_id: describe.map(|describe| describe.logical_id.clone()),
                name: describe.map(|describe| describe.name.clone()),
                abi_version: describe
                    .map(|describe| describe.abi_version)
                    .filter(|abi_version| *abi_version != 0),
            },
            outcome: completion.outcome.clone(),
            host_decision: host_decision.clone(),
            explanation: explanation_for_outcome(
                &completion.outcome,
                &host_decision,
                escalation.as_ref(),
                finished_at_ms,
            ),
            escalation,
            artifacts,
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
        self.write_runtime_error_with_context(error, MissionRecordContext::default())
    }

    pub(crate) fn write_runtime_error_with_context(
        &self,
        error: &anyhow::Error,
        context: MissionRecordContext,
    ) -> Result<()> {
        let finished_at_ms = now_ms();
        let outcome = if error_category(error) == "timeout" {
            MissionOutcome {
                status: MissionOutcomeStatus::RetryableFailure,
                reason_code: "acquisition_timeout".to_string(),
                message: format!("{error:#}"),
                retry_after_ms: None,
                operator_action: None,
                operator_timeout_ms: None,
                operator_timeout_outcome: None,
                primary_artifact_kind: None,
            }
        } else {
            MissionOutcome {
                status: MissionOutcomeStatus::TerminalFailure,
                reason_code: error_category(error).to_string(),
                message: format!("{error:#}"),
                retry_after_ms: None,
                operator_action: None,
                operator_timeout_ms: None,
                operator_timeout_outcome: None,
                primary_artifact_kind: None,
            }
        };
        let host_decision = host_decision_record(fallback_error_decision(error), &outcome, true);
        let record = MissionRecord {
            schema_version: RESULT_SCHEMA_VERSION,
            record_kind: MissionRecordKind::Result,
            job: context.job.or_else(|| Some(self.default_job_record())),
            mission: context
                .mission
                .or_else(|| Some(mission_contract_record(self.wasm_path.as_deref(), None))),
            attempt: context.attempt.or_else(|| {
                Some(default_attempt_record(
                    1,
                    None,
                    None,
                    self.started_at.clone(),
                    finished_at_ms,
                    true,
                ))
            }),
            timeline: context.timeline,
            challenges: context.challenges,
            interventions: context.interventions,
            payload: None,
            module: MissionModuleRecord {
                wasm_path: self.wasm_path.clone(),
                logical_id: None,
                name: None,
                abi_version: None,
            },
            outcome: outcome.clone(),
            host_decision: host_decision.clone(),
            explanation: explanation_for_outcome(
                &outcome,
                &host_decision,
                None,
                finished_at_ms,
            ),
            escalation: None,
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

    fn default_job_record(&self) -> MissionJobRecord {
        MissionJobRecord {
            mode: self.mode,
            name: None,
            scheduler_state: None,
            held: false,
            cycles: 0,
            files: MissionJobFilesRecord {
                result_path: self.path.display().to_string(),
                status_path: None,
                events_path: None,
            },
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct MissionRecordContext {
    pub(crate) job: Option<MissionJobRecord>,
    pub(crate) mission: Option<MissionContractRecord>,
    pub(crate) attempt: Option<MissionAttemptRecord>,
    pub(crate) timeline: Vec<MissionTimelineEntry>,
    pub(crate) challenges: Vec<MissionChallengeRecord>,
    pub(crate) interventions: Vec<MissionInterventionRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissionRecord {
    pub(crate) schema_version: u8,
    #[serde(default)]
    pub(crate) record_kind: MissionRecordKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) job: Option<MissionJobRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) mission: Option<MissionContractRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) attempt: Option<MissionAttemptRecord>,
    #[serde(default)]
    pub(crate) timeline: Vec<MissionTimelineEntry>,
    #[serde(default)]
    pub(crate) challenges: Vec<MissionChallengeRecord>,
    #[serde(default)]
    pub(crate) interventions: Vec<MissionInterventionRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) payload: Option<MissionPayloadRecord>,
    pub(crate) module: MissionModuleRecord,
    pub(crate) outcome: MissionOutcome,
    pub(crate) host_decision: HostDecisionRecord,
    pub(crate) explanation: ExplanationRecord,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) escalation: Option<MissionEscalationRecord>,
    #[serde(default)]
    pub(crate) artifacts: MissionArtifactsRecord,
    pub(crate) timing: TimingRecord,
    pub(crate) stats: MissionStatsRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissionStatusRecord {
    pub(crate) schema_version: u8,
    #[serde(default)]
    pub(crate) record_kind: MissionRecordKind,
    pub(crate) job: MissionJobRecord,
    pub(crate) mission: MissionContractRecord,
    pub(crate) module: MissionModuleRecord,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) attempt: Option<MissionAttemptRecord>,
    #[serde(default)]
    pub(crate) timeline: Vec<MissionTimelineEntry>,
    #[serde(default)]
    pub(crate) challenges: Vec<MissionChallengeRecord>,
    #[serde(default)]
    pub(crate) interventions: Vec<MissionInterventionRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) outcome: Option<MissionOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) host_decision: Option<HostDecisionRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) explanation: Option<ExplanationRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) escalation: Option<MissionEscalationRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) last_error: Option<String>,
    #[serde(default)]
    pub(crate) stats: MissionStatsRecord,
    pub(crate) updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissionJobRecord {
    pub(crate) mode: MissionJobMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) scheduler_state: Option<String>,
    #[serde(default)]
    pub(crate) held: bool,
    #[serde(default)]
    pub(crate) cycles: u64,
    pub(crate) files: MissionJobFilesRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissionJobFilesRecord {
    pub(crate) result_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) status_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) events_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MissionContractRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) wasm_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) logical_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) abi_version: Option<u32>,
    #[serde(default)]
    pub(crate) run_modes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) state_persistence: Option<PersistenceAuthority>,
    #[serde(default)]
    pub(crate) required_env_vars: Vec<EnvVarSpec>,
    #[serde(default)]
    pub(crate) optional_env_vars: Vec<EnvVarSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) params: Option<MissionParamsSchema>,
    #[serde(default)]
    pub(crate) capabilities_needed: Vec<String>,
    #[serde(default)]
    pub(crate) artifact_types: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissionAttemptRecord {
    pub(crate) sequence: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) scheduler_state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) phase: Option<String>,
    pub(crate) started_at: String,
    pub(crate) updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) finished_at: Option<String>,
    #[serde(default)]
    pub(crate) terminal: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissionTimelineEntry {
    pub(crate) ts: String,
    pub(crate) ts_ms: u64,
    pub(crate) kind: String,
    pub(crate) summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) scheduler_state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) phase: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) outcome_status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissionChallengeRecord {
    pub(crate) ts: String,
    pub(crate) ts_ms: u64,
    pub(crate) kind: String,
    pub(crate) summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissionInterventionRecord {
    pub(crate) ts: String,
    pub(crate) ts_ms: u64,
    pub(crate) actor: String,
    pub(crate) action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissionPayloadRecord {
    pub(crate) kind: String,
    pub(crate) size_bytes: usize,
    pub(crate) encoding: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) media_type: Option<String>,
    pub(crate) base64: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) json: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MissionModuleRecord {
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
pub struct HostDecisionRecord {
    #[serde(default)]
    pub(crate) exit_code: i32,
    #[serde(default)]
    pub(crate) category: String,
    #[serde(default)]
    pub(crate) synthesized: bool,
    #[serde(default)]
    pub(crate) risk_posture: MissionRiskPosture,
    #[serde(default)]
    pub(crate) next_attempt_policy: NextAttemptPolicy,
    #[serde(default)]
    pub(crate) basis: Vec<DecisionBasisTag>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExplanationRecord {
    pub(crate) summary: String,
    pub(crate) message: String,
    pub(crate) next_action: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissionEscalationRecord {
    pub(crate) action: String,
    pub(crate) deadline_at: String,
    pub(crate) deadline_at_ms: u64,
    pub(crate) timeout_outcome: OperatorTimeoutOutcome,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MissionArtifactsRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) raw_source: Option<MissionArtifactRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) normalized: Option<MissionArtifactRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) published_output: Option<MissionArtifactRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissionArtifactRecord {
    pub(crate) size_bytes: usize,
    pub(crate) base64: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) json: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimingRecord {
    pub(crate) started_at: String,
    pub(crate) finished_at: String,
    pub(crate) elapsed_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MissionStatsRecord {
    pub(crate) consecutive_failures: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) last_success_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) last_failure_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) cooldown_until_ms: Option<u64>,
}

pub fn load_record(path: &Path) -> Result<MissionRecord> {
    let bytes =
        std::fs::read(path).with_context(|| format!("read mission record {}", path.display()))?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("decode mission record {}", path.display()))
}

pub fn write_status_record(path: &Path, record: &MissionStatusRecord) -> Result<()> {
    let bytes =
        serde_json::to_vec_pretty(record).context("serialize mission status record as JSON")?;
    atomic_write(path, &bytes)
}

#[derive(Debug, Clone, Serialize)]
pub struct MissionExplainView {
    pub(crate) summary: String,
    pub(crate) outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) recorded_outcome: Option<String>,
    pub(crate) reason_code: String,
    pub(crate) message: String,
    pub(crate) next_action: String,
    pub(crate) exit_code: i32,
    pub(crate) category: String,
    pub(crate) synthesized: bool,
    pub(crate) risk_posture: String,
    pub(crate) next_attempt_policy: String,
    pub(crate) basis: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) deadline_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) timeout_outcome: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) rescue_window_open: Option<bool>,
    pub(crate) consecutive_failures: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) last_success_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) cooldown_until: Option<String>,
    pub(crate) started_at: String,
    pub(crate) finished_at: String,
}

pub fn explain_record(record: &MissionRecord, now_ms: u64) -> MissionExplainView {
    let analysis = explain_analysis(
        &record.outcome,
        &record.host_decision,
        record.escalation.as_ref(),
        now_ms,
    );
    let effective_status = analysis.effective_status;
    let outcome = status_name(effective_status).to_string();
    let recorded_outcome = (effective_status != record.outcome.status)
        .then(|| status_name(record.outcome.status).to_string());
    MissionExplainView {
        summary: analysis.summary,
        outcome,
        recorded_outcome,
        reason_code: record.outcome.reason_code.clone(),
        message: record.outcome.message.clone(),
        next_action: analysis.next_action,
        exit_code: analysis.host_decision.exit_code,
        category: analysis.host_decision.category.clone(),
        synthesized: analysis.host_decision.synthesized,
        risk_posture: enum_name(&analysis.host_decision.risk_posture),
        next_attempt_policy: enum_name(&analysis.host_decision.next_attempt_policy),
        basis: analysis.host_decision.basis.iter().map(enum_name).collect(),
        deadline_at: record
            .escalation
            .as_ref()
            .map(|escalation| escalation.deadline_at.clone()),
        timeout_outcome: record
            .escalation
            .as_ref()
            .map(|escalation| status_name(escalation.timeout_outcome.mission_status()).to_string()),
        rescue_window_open: analysis.rescue_window_open,
        consecutive_failures: record.stats.consecutive_failures,
        last_success_at: record.stats.last_success_at_ms.map(ms_to_iso8601),
        cooldown_until: record.stats.cooldown_until_ms.map(ms_to_iso8601),
        started_at: record.timing.started_at.clone(),
        finished_at: record.timing.finished_at.clone(),
    }
}

pub fn mission_contract_record(
    wasm_path: Option<&str>,
    describe: Option<&MissionModuleDescribe>,
) -> MissionContractRecord {
    MissionContractRecord {
        wasm_path: wasm_path.map(ToOwned::to_owned),
        logical_id: describe.map(|describe| describe.logical_id.clone()),
        name: describe.map(|describe| describe.name.clone()),
        abi_version: describe
            .map(|describe| describe.abi_version)
            .filter(|abi_version| *abi_version != 0),
        run_modes: describe
            .map(|describe| describe.run_modes.clone())
            .unwrap_or_default(),
        state_persistence: describe.map(|describe| describe.state_persistence.clone()),
        required_env_vars: describe
            .map(|describe| describe.required_env_vars.clone())
            .unwrap_or_default(),
        optional_env_vars: describe
            .map(|describe| describe.optional_env_vars.clone())
            .unwrap_or_default(),
        params: describe.and_then(|describe| describe.params.clone()),
        capabilities_needed: describe
            .map(|describe| describe.capabilities_needed.clone())
            .unwrap_or_default(),
        artifact_types: describe
            .map(|describe| describe.artifact_types.clone())
            .unwrap_or_default(),
    }
}

fn default_attempt_record(
    sequence: u64,
    scheduler_state: Option<String>,
    phase: Option<String>,
    started_at: String,
    finished_at_ms: u64,
    terminal: bool,
) -> MissionAttemptRecord {
    let finished_at = ms_to_iso8601(finished_at_ms);
    MissionAttemptRecord {
        sequence,
        scheduler_state,
        phase,
        started_at,
        updated_at: finished_at.clone(),
        finished_at: terminal.then_some(finished_at),
        terminal,
    }
}

fn phase_name_from_completion(completion: &MissionCompletion) -> String {
    enum_name(&completion.snapshot.phase)
}

fn payload_record(outcome: &MissionOutcome, data: &[u8]) -> MissionPayloadRecord {
    let json = serde_json::from_slice::<serde_json::Value>(data).ok();
    let text = std::str::from_utf8(data).ok().map(ToOwned::to_owned);
    MissionPayloadRecord {
        kind: outcome
            .primary_artifact_kind
            .clone()
            .unwrap_or_else(|| "published_output".to_string()),
        size_bytes: data.len(),
        encoding: "base64".to_string(),
        media_type: if json.is_some() {
            Some("application/json".to_string())
        } else if text.is_some() {
            Some("text/plain; charset=utf-8".to_string())
        } else {
            None
        },
        base64: STANDARD.encode(data),
        json,
        text,
    }
}

fn artifact_record(data: &[u8]) -> MissionArtifactRecord {
    MissionArtifactRecord {
        size_bytes: data.len(),
        base64: STANDARD.encode(data),
        json: serde_json::from_slice::<serde_json::Value>(data).ok(),
        text: std::str::from_utf8(data).ok().map(ToOwned::to_owned),
    }
}

pub(crate) fn escalation_record(escalation: &OperatorEscalationState) -> MissionEscalationRecord {
    MissionEscalationRecord {
        action: escalation.action.clone(),
        deadline_at: ms_to_iso8601(escalation.deadline_at_ms),
        deadline_at_ms: escalation.deadline_at_ms,
        timeout_outcome: escalation.timeout_outcome,
    }
}

fn explanation_for_outcome(
    outcome: &MissionOutcome,
    host_decision: &HostDecisionRecord,
    escalation: Option<&MissionEscalationRecord>,
    now_ms: u64,
) -> ExplanationRecord {
    let analysis = explain_analysis(outcome, host_decision, escalation, now_ms);
    ExplanationRecord {
        summary: analysis.summary,
        message: outcome.message.clone(),
        next_action: analysis.next_action,
    }
}

const fn category_for_status(status: MissionOutcomeStatus) -> &'static str {
    match status {
        MissionOutcomeStatus::Published => "published",
        MissionOutcomeStatus::RetryableFailure => "retryable_failure",
        MissionOutcomeStatus::TerminalFailure => "terminal_failure",
        MissionOutcomeStatus::OperatorActionRequired => "operator_action_required",
    }
}

const fn exit_code_for_status(status: MissionOutcomeStatus) -> i32 {
    match status {
        MissionOutcomeStatus::Published => 0,
        MissionOutcomeStatus::RetryableFailure => 75,
        MissionOutcomeStatus::TerminalFailure => 70,
        MissionOutcomeStatus::OperatorActionRequired => 65,
    }
}

const fn status_name(status: MissionOutcomeStatus) -> &'static str {
    match status {
        MissionOutcomeStatus::Published => "published",
        MissionOutcomeStatus::RetryableFailure => "retryable_failure",
        MissionOutcomeStatus::TerminalFailure => "terminal_failure",
        MissionOutcomeStatus::OperatorActionRequired => "operator_action_required",
    }
}

fn exit_code_for_outcome(outcome: &MissionOutcome) -> i32 {
    if outcome.reason_code == "acquisition_timeout" {
        return 124;
    }
    exit_code_for_status(outcome.status)
}

#[derive(Debug, Clone)]
struct ExplainAnalysis {
    effective_status: MissionOutcomeStatus,
    host_decision: HostDecisionRecord,
    summary: String,
    next_action: String,
    rescue_window_open: Option<bool>,
}

fn explain_analysis(
    outcome: &MissionOutcome,
    host_decision: &HostDecisionRecord,
    escalation: Option<&MissionEscalationRecord>,
    now_ms: u64,
) -> ExplainAnalysis {
    match outcome.status {
        MissionOutcomeStatus::Published => published_analysis(outcome, host_decision),
        MissionOutcomeStatus::RetryableFailure => retryable_analysis(outcome, host_decision),
        MissionOutcomeStatus::TerminalFailure => terminal_analysis(outcome, host_decision),
        MissionOutcomeStatus::OperatorActionRequired => {
            operator_action_analysis(outcome, host_decision, escalation, now_ms)
        }
    }
}

fn published_analysis(
    outcome: &MissionOutcome,
    host_decision: &HostDecisionRecord,
) -> ExplainAnalysis {
    ExplainAnalysis {
        effective_status: MissionOutcomeStatus::Published,
        host_decision: host_decision.clone(),
        summary: format!(
            "Mission published {}.",
            outcome
                .primary_artifact_kind
                .as_deref()
                .unwrap_or("its final artifact")
        ),
        next_action: "Watch the mission result file and consume payload.base64 or payload.json."
            .to_string(),
        rescue_window_open: None,
    }
}

fn retryable_analysis(
    outcome: &MissionOutcome,
    host_decision: &HostDecisionRecord,
) -> ExplainAnalysis {
    ExplainAnalysis {
        effective_status: MissionOutcomeStatus::RetryableFailure,
        host_decision: host_decision.clone(),
        summary: format!(
            "Mission failed with a retryable condition: {}.",
            outcome.reason_code
        ),
        next_action: if host_decision.next_attempt_policy == NextAttemptPolicy::ManualOnly
            || host_decision.risk_posture == MissionRiskPosture::AwaitingChangedConditions
        {
            "Change the inputs, environment, or module before launching another automated attempt."
                .to_string()
        } else {
            outcome.retry_after_ms.map_or_else(
                || "Retry when the orchestration policy allows.".to_string(),
                |retry_after_ms| format!("Retry after {retry_after_ms} ms."),
            )
        },
        rescue_window_open: None,
    }
}

fn terminal_analysis(
    outcome: &MissionOutcome,
    host_decision: &HostDecisionRecord,
) -> ExplainAnalysis {
    ExplainAnalysis {
        effective_status: MissionOutcomeStatus::TerminalFailure,
        host_decision: host_decision.clone(),
        summary: format!("Mission failed terminally: {}.", outcome.reason_code),
        next_action: "Do not retry automatically; inspect the mission explanation.".to_string(),
        rescue_window_open: None,
    }
}

fn operator_action_analysis(
    outcome: &MissionOutcome,
    host_decision: &HostDecisionRecord,
    escalation: Option<&MissionEscalationRecord>,
    now_ms: u64,
) -> ExplainAnalysis {
    match escalation {
        Some(escalation) if now_ms <= escalation.deadline_at_ms => {
            operator_window_open_analysis(host_decision, escalation)
        }
        Some(escalation) => operator_window_expired_analysis(host_decision, escalation),
        None => operator_action_required_analysis(outcome, host_decision),
    }
}

fn operator_window_open_analysis(
    host_decision: &HostDecisionRecord,
    escalation: &MissionEscalationRecord,
) -> ExplainAnalysis {
    ExplainAnalysis {
        effective_status: MissionOutcomeStatus::OperatorActionRequired,
        host_decision: host_decision.clone(),
        summary: format!(
            "Mission is awaiting operator rescue until {}.",
            escalation.deadline_at
        ),
        next_action: format!(
            "{} Rescue window closes at {}.",
            escalation.action, escalation.deadline_at
        ),
        rescue_window_open: Some(true),
    }
}

fn operator_window_expired_analysis(
    host_decision: &HostDecisionRecord,
    escalation: &MissionEscalationRecord,
) -> ExplainAnalysis {
    let effective_status = escalation.timeout_outcome.mission_status();
    let next_action = match effective_status {
        MissionOutcomeStatus::RetryableFailure => {
            "Start a new mission attempt when orchestration policy allows.".to_string()
        }
        MissionOutcomeStatus::TerminalFailure => {
            "Do not retry automatically; fix prerequisites before launching a new attempt."
                .to_string()
        }
        _ => unreachable!("operator timeout outcomes are terminal"),
    };
    let mut effective_host_decision = host_decision.clone();
    effective_host_decision.exit_code = exit_code_for_status(effective_status);
    effective_host_decision.category = category_for_status(effective_status).to_string();
    effective_host_decision.risk_posture = MissionRiskPosture::ClosedSafe;
    effective_host_decision.next_attempt_policy = match effective_status {
        MissionOutcomeStatus::RetryableFailure => NextAttemptPolicy::AfterCooldown,
        MissionOutcomeStatus::TerminalFailure => NextAttemptPolicy::ManualOnly,
        _ => effective_host_decision.next_attempt_policy,
    };
    if !effective_host_decision
        .basis
        .contains(&DecisionBasisTag::RescueWindowExpired)
    {
        effective_host_decision
            .basis
            .push(DecisionBasisTag::RescueWindowExpired);
    }
    ExplainAnalysis {
        effective_status,
        host_decision: effective_host_decision,
        summary: format!(
            "Operator rescue window expired at {}; closing the attempt as {}.",
            escalation.deadline_at,
            status_name(effective_status)
        ),
        next_action,
        rescue_window_open: Some(false),
    }
}

fn operator_action_required_analysis(
    outcome: &MissionOutcome,
    host_decision: &HostDecisionRecord,
) -> ExplainAnalysis {
    ExplainAnalysis {
        effective_status: MissionOutcomeStatus::OperatorActionRequired,
        host_decision: host_decision.clone(),
        summary: format!("Mission needs operator action: {}.", outcome.reason_code),
        next_action: outcome
            .operator_action
            .clone()
            .unwrap_or_else(|| "Perform the required operator action before retrying.".to_string()),
        rescue_window_open: None,
    }
}

pub fn host_decision_record(
    decision: HostDecisionState,
    outcome: &MissionOutcome,
    durable_record_written: bool,
) -> HostDecisionRecord {
    let mut basis = decision.basis.clone();
    if durable_record_written && !basis.contains(&DecisionBasisTag::DurableRecordWritten) {
        basis.push(DecisionBasisTag::DurableRecordWritten);
    }
    HostDecisionRecord {
        exit_code: exit_code_for_outcome(outcome),
        category: decision.category,
        synthesized: decision.synthesized,
        risk_posture: decision.risk_posture,
        next_attempt_policy: decision.next_attempt_policy,
        basis,
    }
}

pub fn fallback_host_decision(outcome: &MissionOutcome, synthesized: bool) -> HostDecisionState {
    let mut basis = if synthesized {
        vec![DecisionBasisTag::HostSynthesized]
    } else {
        Vec::new()
    };
    let (category, risk_posture, next_attempt_policy) = match outcome.status {
        MissionOutcomeStatus::Published => {
            basis.push(DecisionBasisTag::ObjectiveMet);
            (
                "published".to_string(),
                MissionRiskPosture::Nominal,
                NextAttemptPolicy::None,
            )
        }
        MissionOutcomeStatus::RetryableFailure => {
            basis.push(DecisionBasisTag::ObjectiveNotMet);
            basis.push(DecisionBasisTag::SafeStateEntered);
            if outcome.reason_code == "changed_conditions_required" {
                basis.push(DecisionBasisTag::ChangedConditionsRequired);
                (
                    "retryable_failure".to_string(),
                    MissionRiskPosture::AwaitingChangedConditions,
                    NextAttemptPolicy::ManualOnly,
                )
            } else {
                if outcome.retry_after_ms.is_some() || outcome.reason_code == "acquisition_timeout"
                {
                    basis.push(DecisionBasisTag::CooldownApplied);
                }
                (
                    if outcome.reason_code == "acquisition_timeout" {
                        "timeout".to_string()
                    } else {
                        "retryable_failure".to_string()
                    },
                    MissionRiskPosture::Degraded,
                    NextAttemptPolicy::AfterCooldown,
                )
            }
        }
        MissionOutcomeStatus::TerminalFailure => {
            basis.push(DecisionBasisTag::ObjectiveNotMet);
            basis.push(DecisionBasisTag::SafeStateEntered);
            (
                "terminal_failure".to_string(),
                MissionRiskPosture::ClosedSafe,
                NextAttemptPolicy::ManualOnly,
            )
        }
        MissionOutcomeStatus::OperatorActionRequired => {
            basis.push(DecisionBasisTag::ObjectiveNotMet);
            basis.push(DecisionBasisTag::AutomationExhausted);
            basis.push(DecisionBasisTag::OperatorRescueOpened);
            (
                "operator_action_required".to_string(),
                MissionRiskPosture::AwaitingOperator,
                NextAttemptPolicy::OperatorRescue,
            )
        }
    };
    HostDecisionState {
        category,
        synthesized,
        risk_posture,
        next_attempt_policy,
        basis,
    }
}

fn fallback_error_decision(error: &anyhow::Error) -> HostDecisionState {
    let outcome = if error_category(error) == "timeout" {
        MissionOutcome {
            status: MissionOutcomeStatus::RetryableFailure,
            reason_code: "acquisition_timeout".to_string(),
            message: format!("{error:#}"),
            retry_after_ms: None,
            operator_action: None,
            operator_timeout_ms: None,
            operator_timeout_outcome: None,
            primary_artifact_kind: None,
        }
    } else {
        MissionOutcome {
            status: MissionOutcomeStatus::TerminalFailure,
            reason_code: error_category(error).to_string(),
            message: format!("{error:#}"),
            retry_after_ms: None,
            operator_action: None,
            operator_timeout_ms: None,
            operator_timeout_outcome: None,
            primary_artifact_kind: None,
        }
    };
    fallback_host_decision(&outcome, true)
}

fn enum_name<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value)
        .unwrap_or_else(|_| "\"unknown\"".to_string())
        .trim_matches('"')
        .to_string()
}

fn error_category(error: &anyhow::Error) -> &'static str {
    error
        .downcast_ref::<BrrmmmmError>()
        .map_or("unexpected", |error| error.category().as_str())
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
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
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
