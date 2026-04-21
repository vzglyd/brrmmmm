use std::path::Path;

use anyhow::Result;

use crate::cli::OutputFormat;
use crate::mission_result::{explain_record, load_record};

use brrmmmm::events::now_ms;

use super::output::print_table;

pub(crate) fn cmd_explain(record_path: &Path, output: OutputFormat) -> Result<()> {
    let record = load_record(record_path)?;
    let view = explain_record(&record, now_ms());

    match output {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&view)?);
        }
        OutputFormat::Text => {
            println!("summary:             {}", view.summary);
            println!("outcome:             {}", view.outcome);
            if let Some(recorded_outcome) = &view.recorded_outcome {
                println!("recorded:            {recorded_outcome}");
            }
            println!("reason_code:         {}", view.reason_code);
            println!("message:             {}", view.message);
            println!("next_action:         {}", view.next_action);
            println!("risk_posture:        {}", view.risk_posture);
            println!("next_policy:         {}", view.next_attempt_policy);
            if !view.basis.is_empty() {
                println!("basis:               {}", view.basis.join(", "));
            }
            if let Some(deadline_at) = &view.deadline_at {
                println!("deadline_at:         {deadline_at}");
            }
            if let Some(timeout_outcome) = &view.timeout_outcome {
                println!("timeout_as:          {timeout_outcome}");
            }
            if let Some(rescue_window_open) = view.rescue_window_open {
                println!("rescue_open:         {rescue_window_open}");
            }
            println!("consecutive_failures:{}", view.consecutive_failures);
            println!(
                "last_success_at:     {}",
                view.last_success_at.as_deref().unwrap_or("(none)")
            );
            println!(
                "cooldown_until:      {}",
                view.cooldown_until.as_deref().unwrap_or("(none)")
            );
            println!("exit_code:           {}", view.exit_code);
            println!("category:            {}", view.category);
            println!("synthesized:         {}", view.synthesized);
            println!("started_at:          {}", view.started_at);
            println!("finished_at:         {}", view.finished_at);
        }
        OutputFormat::Table => {
            let mut rows = vec![
                ("summary", view.summary),
                ("outcome", view.outcome),
                ("reason_code", view.reason_code),
                ("message", view.message),
                ("next_action", view.next_action),
                ("risk_posture", view.risk_posture),
                ("next_policy", view.next_attempt_policy),
                ("exit_code", view.exit_code.to_string()),
                ("category", view.category),
                ("synthesized", view.synthesized.to_string()),
            ];
            if !view.basis.is_empty() {
                rows.push(("basis", view.basis.join(", ")));
            }
            if let Some(recorded_outcome) = view.recorded_outcome {
                rows.push(("recorded", recorded_outcome));
            }
            if let Some(deadline_at) = view.deadline_at {
                rows.push(("deadline_at", deadline_at));
            }
            if let Some(timeout_outcome) = view.timeout_outcome {
                rows.push(("timeout_as", timeout_outcome));
            }
            if let Some(rescue_window_open) = view.rescue_window_open {
                rows.push(("rescue_open", rescue_window_open.to_string()));
            }
            rows.push((
                "consecutive_failures",
                view.consecutive_failures.to_string(),
            ));
            rows.push((
                "last_success_at",
                view.last_success_at.unwrap_or_else(|| "(none)".to_string()),
            ));
            rows.push((
                "cooldown_until",
                view.cooldown_until.unwrap_or_else(|| "(none)".to_string()),
            ));
            print_table(&rows);
        }
    }

    Ok(())
}
