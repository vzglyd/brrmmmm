use std::path::Path;

use anyhow::Result;
use serde::Serialize;

use brrmmmm::abi::{MissionOutcome, MissionOutcomeStatus};
use brrmmmm::config::Config;
use brrmmmm::controller::inspect_module_contract;
use brrmmmm::events::{ms_to_iso8601, now_ms};

use crate::cli::OutputFormat;
use crate::mission_result::{
    ExplanationRecord, MissionArtifactsRecord, MissionEscalationRecord, MissionExplainView,
    MissionModuleRecord, MissionRecord, MissionStatsRecord, TimingRecord, explain_record,
    fallback_host_decision, host_decision_record,
};

use super::output::print_table;

#[derive(Serialize)]
struct RehearsalScenario {
    scenario: String,
    record: MissionRecord,
    explain: MissionExplainView,
}

pub(crate) fn cmd_rehearse(wasm_path: &Path, output: OutputFormat, config: &Config) -> Result<()> {
    let wasm_str = wasm_path.to_string_lossy();
    let inspection = inspect_module_contract(&wasm_str)?;
    let describe = inspection.describe.as_ref();
    let now = now_ms();

    let mut scenarios = vec![
        rehearsal_scenario(
            "published",
            module_record(&wasm_str, describe, inspection.abi_version),
            MissionOutcome {
                status: MissionOutcomeStatus::Published,
                reason_code: "published_output".to_string(),
                message: "rehearsal: mission module published its final artifact".to_string(),
                retry_after_ms: None,
                operator_action: None,
                operator_timeout_ms: None,
                operator_timeout_outcome: None,
                primary_artifact_kind: Some("published_output".to_string()),
            },
            None,
            now,
        ),
        rehearsal_scenario(
            "retryable_failure",
            module_record(&wasm_str, describe, inspection.abi_version),
            MissionOutcome {
                status: MissionOutcomeStatus::RetryableFailure,
                reason_code: "source_unavailable".to_string(),
                message:
                    "rehearsal: mission module reported a retryable failure; runtime entered safe state with cooldown"
                        .to_string(),
                retry_after_ms: Some(config.assurance.default_retry_after_ms),
                operator_action: None,
                operator_timeout_ms: None,
                operator_timeout_outcome: None,
                primary_artifact_kind: None,
            },
            None,
            now,
        ),
        rehearsal_scenario(
            "timeout",
            module_record(&wasm_str, describe, inspection.abi_version),
            MissionOutcome {
                status: MissionOutcomeStatus::RetryableFailure,
                reason_code: "acquisition_timeout".to_string(),
                message: "rehearsal: acquisition budget expired before mission completion"
                    .to_string(),
                retry_after_ms: Some(config.assurance.default_retry_after_ms),
                operator_action: None,
                operator_timeout_ms: None,
                operator_timeout_outcome: None,
                primary_artifact_kind: None,
            },
            None,
            now,
        ),
        rehearsal_scenario(
            "protocol_error",
            module_record(&wasm_str, describe, inspection.abi_version),
            MissionOutcome {
                status: MissionOutcomeStatus::TerminalFailure,
                reason_code: "mission_protocol_error".to_string(),
                message:
                    "rehearsal: mission module violated the host contract and was closed safely"
                        .to_string(),
                retry_after_ms: None,
                operator_action: None,
                operator_timeout_ms: None,
                operator_timeout_outcome: None,
                primary_artifact_kind: None,
            },
            None,
            now,
        ),
        rehearsal_scenario(
            "repeat_failure_gate",
            module_record(&wasm_str, describe, inspection.abi_version),
            MissionOutcome {
                status: MissionOutcomeStatus::RetryableFailure,
                reason_code: "changed_conditions_required".to_string(),
                message:
                    "rehearsal: unchanged inputs triggered the repeat-failure gate before execution"
                        .to_string(),
                retry_after_ms: None,
                operator_action: None,
                operator_timeout_ms: None,
                operator_timeout_outcome: None,
                primary_artifact_kind: None,
            },
            None,
            now,
        ),
    ];

    if let Some(fallback) = describe.and_then(|describe| describe.operator_fallback.as_ref()) {
        let deadline_at_ms = now.saturating_sub(1_000);
        scenarios.push(rehearsal_scenario(
            "operator_rescue_expired",
            module_record(&wasm_str, describe, inspection.abi_version),
            MissionOutcome {
                status: MissionOutcomeStatus::OperatorActionRequired,
                reason_code: "operator_rescue_rehearsal".to_string(),
                message: "rehearsal: automation exhausted and the operator rescue window expired"
                    .to_string(),
                retry_after_ms: None,
                operator_action: Some(
                    "Complete the upstream recovery action before another attempt.".to_string(),
                ),
                operator_timeout_ms: Some(fallback.timeout_ms),
                operator_timeout_outcome: Some(fallback.on_timeout),
                primary_artifact_kind: None,
            },
            Some(MissionEscalationRecord {
                action: "Complete the upstream recovery action before another attempt.".to_string(),
                deadline_at: ms_to_iso8601(deadline_at_ms),
                deadline_at_ms,
                timeout_outcome: fallback.on_timeout,
            }),
            now,
        ));
    }

    match output {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&scenarios)?);
        }
        OutputFormat::Text => {
            for scenario in &scenarios {
                println!("scenario:      {}", scenario.scenario);
                println!("summary:       {}", scenario.explain.summary);
                println!("outcome:       {}", scenario.explain.outcome);
                println!("risk_posture:  {}", scenario.explain.risk_posture);
                println!("next_policy:   {}", scenario.explain.next_attempt_policy);
                if !scenario.explain.basis.is_empty() {
                    println!("basis:         {}", scenario.explain.basis.join(", "));
                }
                println!("next_action:   {}", scenario.explain.next_action);
                println!();
            }
        }
        OutputFormat::Table => {
            let rows = scenarios
                .iter()
                .map(|scenario| {
                    (
                        scenario.scenario.as_str(),
                        format!(
                            "{} | {} | {}",
                            scenario.explain.outcome,
                            scenario.explain.risk_posture,
                            scenario.explain.next_attempt_policy
                        ),
                    )
                })
                .collect::<Vec<_>>();
            print_table(&rows);
        }
    }

    Ok(())
}

fn rehearsal_scenario(
    scenario: &str,
    module: MissionModuleRecord,
    outcome: MissionOutcome,
    escalation: Option<MissionEscalationRecord>,
    now_ms: u64,
) -> RehearsalScenario {
    let host_decision =
        host_decision_record(fallback_host_decision(&outcome, true), &outcome, false);
    let record = MissionRecord {
        schema_version: 4,
        module,
        outcome: outcome.clone(),
        host_decision: host_decision.clone(),
        explanation: ExplanationRecord {
            summary: format!("rehearsal scenario: {scenario}"),
            message: outcome.message.clone(),
            next_action: "Inspect the explain view for the rehearsed closure path.".to_string(),
        },
        escalation,
        artifacts: MissionArtifactsRecord::default(),
        timing: TimingRecord {
            started_at: ms_to_iso8601(now_ms.saturating_sub(1_000)),
            finished_at: ms_to_iso8601(now_ms),
            elapsed_ms: 1_000,
        },
        stats: MissionStatsRecord::default(),
    };
    let explain = explain_record(&record, now_ms);
    RehearsalScenario {
        scenario: scenario.to_string(),
        record,
        explain,
    }
}

fn module_record(
    wasm_path: &str,
    describe: Option<&brrmmmm::abi::MissionModuleDescribe>,
    abi_version: u32,
) -> MissionModuleRecord {
    MissionModuleRecord {
        wasm_path: Some(wasm_path.to_string()),
        logical_id: describe.map(|describe| describe.logical_id.clone()),
        name: describe.map(|describe| describe.name.clone()),
        abi_version: Some(abi_version).filter(|abi_version| *abi_version != 0),
    }
}
