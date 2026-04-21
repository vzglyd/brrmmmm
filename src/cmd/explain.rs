use std::path::Path;

use anyhow::Result;

use crate::cli::OutputFormat;
use crate::mission_result::load_record;

use super::output::print_table;

pub(crate) fn cmd_explain(record_path: &Path, output: OutputFormat) -> Result<()> {
    let record = load_record(record_path)?;

    match output {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&record)?);
        }
        OutputFormat::Text => {
            println!("summary:       {}", record.explanation.summary);
            println!(
                "outcome:       {}",
                serde_json::to_string(&record.outcome.status)?
                    .trim_matches('"')
                    .to_string()
            );
            println!("reason_code:   {}", record.outcome.reason_code);
            println!("message:       {}", record.explanation.message);
            println!("next_action:   {}", record.explanation.next_action);
            println!("exit_code:     {}", record.host_decision.exit_code);
            println!("category:      {}", record.host_decision.category);
            println!("synthesized:   {}", record.host_decision.synthesized);
            println!("started_at:    {}", record.timing.started_at);
            println!("finished_at:   {}", record.timing.finished_at);
        }
        OutputFormat::Table => {
            let rows = vec![
                ("summary", record.explanation.summary),
                (
                    "outcome",
                    serde_json::to_string(&record.outcome.status)?
                        .trim_matches('"')
                        .to_string(),
                ),
                ("reason_code", record.outcome.reason_code),
                ("message", record.explanation.message),
                ("next_action", record.explanation.next_action),
                ("exit_code", record.host_decision.exit_code.to_string()),
                ("category", record.host_decision.category),
                ("synthesized", record.host_decision.synthesized.to_string()),
            ];
            print_table(&rows);
        }
    }

    Ok(())
}
