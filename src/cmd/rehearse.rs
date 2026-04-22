use std::path::Path;

use anyhow::Result;
use serde::Serialize;

use brrmmmm::abi::{MissionModuleDescribe, MissionOutcome, MissionOutcomeStatus};
use brrmmmm::config::Config;
use brrmmmm::controller::{MissionInspection, inspect_module_contract};
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

pub fn cmd_rehearse(wasm_path: &Path, output: OutputFormat, config: &Config) -> Result<()> {
    let wasm_str = wasm_path.to_string_lossy();
    let inspection = inspect_module_contract(&wasm_str)?;
    let now = now_ms();
    let scenarios = rehearsal_scenarios(&wasm_str, &inspection, config, now);

    match output {
        OutputFormat::Json => print_rehearsal_json(&scenarios)?,
        OutputFormat::Text => print_rehearsal_text(&scenarios),
        OutputFormat::Table => print_rehearsal_table(&scenarios),
    }

    Ok(())
}

fn rehearsal_scenarios(
    wasm_str: &str,
    inspection: &MissionInspection,
    config: &Config,
    now: u64,
) -> Vec<RehearsalScenario> {
    let describe = inspection.describe.as_ref();
    let module = module_record(wasm_str, describe, inspection.abi_version);
    let mut scenarios = vec![
        rehearsal_scenario("published", module.clone(), published_outcome(), None, now),
        rehearsal_scenario(
            "retryable_failure",
            module.clone(),
            retryable_failure_outcome(config),
            None,
            now,
        ),
        rehearsal_scenario(
            "timeout",
            module.clone(),
            timeout_outcome(config),
            None,
            now,
        ),
        rehearsal_scenario(
            "protocol_error",
            module.clone(),
            protocol_error_outcome(),
            None,
            now,
        ),
        rehearsal_scenario(
            "repeat_failure_gate",
            module.clone(),
            repeat_failure_gate_outcome(),
            None,
            now,
        ),
    ];
    append_operator_rescue_scenario(&mut scenarios, module, describe, now);
    scenarios
}

fn published_outcome() -> MissionOutcome {
    MissionOutcome {
        status: MissionOutcomeStatus::Published,
        reason_code: "published_output".to_string(),
        message: "rehearsal: mission module published its final artifact".to_string(),
        retry_after_ms: None,
        operator_action: None,
        operator_timeout_ms: None,
        operator_timeout_outcome: None,
        primary_artifact_kind: Some("published_output".to_string()),
    }
}

fn retryable_failure_outcome(config: &Config) -> MissionOutcome {
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
    }
}

fn timeout_outcome(config: &Config) -> MissionOutcome {
    MissionOutcome {
        status: MissionOutcomeStatus::RetryableFailure,
        reason_code: "acquisition_timeout".to_string(),
        message: "rehearsal: acquisition budget expired before mission completion".to_string(),
        retry_after_ms: Some(config.assurance.default_retry_after_ms),
        operator_action: None,
        operator_timeout_ms: None,
        operator_timeout_outcome: None,
        primary_artifact_kind: None,
    }
}

fn protocol_error_outcome() -> MissionOutcome {
    MissionOutcome {
        status: MissionOutcomeStatus::TerminalFailure,
        reason_code: "mission_protocol_error".to_string(),
        message: "rehearsal: mission module violated the host contract and was closed safely"
            .to_string(),
        retry_after_ms: None,
        operator_action: None,
        operator_timeout_ms: None,
        operator_timeout_outcome: None,
        primary_artifact_kind: None,
    }
}

fn repeat_failure_gate_outcome() -> MissionOutcome {
    MissionOutcome {
        status: MissionOutcomeStatus::RetryableFailure,
        reason_code: "changed_conditions_required".to_string(),
        message: "rehearsal: unchanged inputs triggered the repeat-failure gate before execution"
            .to_string(),
        retry_after_ms: None,
        operator_action: None,
        operator_timeout_ms: None,
        operator_timeout_outcome: None,
        primary_artifact_kind: None,
    }
}

fn append_operator_rescue_scenario(
    scenarios: &mut Vec<RehearsalScenario>,
    module: MissionModuleRecord,
    describe: Option<&MissionModuleDescribe>,
    now: u64,
) {
    let Some(fallback) = describe.and_then(|describe| describe.operator_fallback.as_ref()) else {
        return;
    };
    let deadline_at_ms = now.saturating_sub(1_000);
    scenarios.push(rehearsal_scenario(
        "operator_rescue_expired",
        module,
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

fn print_rehearsal_json(scenarios: &[RehearsalScenario]) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(scenarios)?);
    Ok(())
}

fn print_rehearsal_text(scenarios: &[RehearsalScenario]) {
    for scenario in scenarios {
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

fn print_rehearsal_table(scenarios: &[RehearsalScenario]) {
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
        schema_version: 1,
        record_kind: crate::mission_result::MissionRecordKind::Result,
        job: None,
        mission: None,
        attempt: None,
        timeline: Vec::new(),
        challenges: Vec::new(),
        interventions: Vec::new(),
        payload: None,
        module,
        outcome: outcome.clone(),
        host_decision,
        explanation: ExplanationRecord {
            summary: format!("rehearsal scenario: {scenario}"),
            message: outcome.message,
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
