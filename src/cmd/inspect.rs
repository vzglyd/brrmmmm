use anyhow::Result;
use std::path::Path;

use brrmmmm::config::Config;
use brrmmmm::controller::inspect_module_contract;

use crate::cli::OutputFormat;

use super::output::print_table;

pub(crate) fn cmd_inspect(wasm_path: &Path, output: OutputFormat, config: &Config) -> Result<()> {
    let wasm_str = wasm_path.to_string_lossy();
    let inspection = inspect_module_contract(&wasm_str)?;

    match output {
        OutputFormat::Json => {
            let mut value = serde_json::to_value(&inspection)?;
            value["assurance_defaults"] = serde_json::json!({
                "same_reason_retry_limit": config.assurance.same_reason_retry_limit,
                "default_retry_after_ms": config.assurance.default_retry_after_ms,
            });
            println!("{}", serde_json::to_string_pretty(&value)?);
        }
        OutputFormat::Text => {
            eprintln!("[brrmmmm] inspecting {wasm_str}");
            let describe = inspection.describe.as_ref();
            println!(
                "logical_id:     {}",
                describe
                    .map(|value| value.logical_id.as_str())
                    .unwrap_or("-")
            );
            println!(
                "name:           {}",
                describe.map(|value| value.name.as_str()).unwrap_or("-")
            );
            println!(
                "assurance:     retry_limit={} default_retry_after={} ms",
                config.assurance.same_reason_retry_limit, config.assurance.default_retry_after_ms
            );
            println!("abi_version:    {}", inspection.abi_version);
            println!("size_bytes:     {}", inspection.wasm_size_bytes);
            println!(
                "entrypoint:     {}",
                inspection.entrypoint.as_deref().unwrap_or("-")
            );
            if !inspection.host_imports.is_empty() {
                println!("host_imports:   {}", inspection.host_imports.join(", "));
            }
            if let Some(describe) = describe {
                let persistence = serde_json::to_string(&describe.state_persistence)?;
                let persistence = persistence.trim_matches('"');
                println!("persistence:   {}", persistence);
                println!(
                    "acq_timeout:   {}",
                    describe
                        .acquisition_timeout_secs
                        .map(|secs| format!("{secs}s"))
                        .unwrap_or_else(|| "-".to_string())
                );
                if let Some(fallback) = &describe.operator_fallback {
                    let timeout_outcome = serde_json::to_string(&fallback.on_timeout)?;
                    let timeout_outcome = timeout_outcome.trim_matches('"');
                    println!("operator_ttl:  {} ms", fallback.timeout_ms);
                    println!("operator_on:   {}", timeout_outcome);
                }
                if let Some(poll) = &describe.poll_strategy {
                    println!("poll_strategy:  {poll}");
                }
                println!("artifacts:      {}", describe.artifact_types.join(", "));
                if !describe.optional_env_vars.is_empty() {
                    let names: Vec<&str> = describe
                        .optional_env_vars
                        .iter()
                        .map(|env| env.name.as_str())
                        .collect();
                    println!("optional_env:   {}", names.join(", "));
                }
            }
        }
        OutputFormat::Table => {
            let describe = inspection.describe.as_ref();
            let mut rows: Vec<(&str, String)> = vec![
                (
                    "logical_id",
                    describe
                        .map(|value| value.logical_id.clone())
                        .unwrap_or_default(),
                ),
                (
                    "name",
                    describe.map(|value| value.name.clone()).unwrap_or_default(),
                ),
                (
                    "assurance",
                    format!(
                        "retry_limit={} default_retry_after={} ms",
                        config.assurance.same_reason_retry_limit,
                        config.assurance.default_retry_after_ms
                    ),
                ),
                ("abi_version", inspection.abi_version.to_string()),
                ("size_bytes", inspection.wasm_size_bytes.to_string()),
                (
                    "entrypoint",
                    inspection.entrypoint.clone().unwrap_or_default(),
                ),
            ];
            if !inspection.host_imports.is_empty() {
                rows.push(("host_imports", inspection.host_imports.join(", ")));
            }
            if let Some(describe) = describe {
                rows.push((
                    "persistence",
                    serde_json::to_string(&describe.state_persistence)?
                        .trim_matches('"')
                        .to_string(),
                ));
                rows.push((
                    "acq_timeout",
                    describe
                        .acquisition_timeout_secs
                        .map(|secs| format!("{secs}s"))
                        .unwrap_or_default(),
                ));
                if let Some(fallback) = &describe.operator_fallback {
                    rows.push(("operator_ttl", format!("{} ms", fallback.timeout_ms)));
                    rows.push((
                        "operator_on",
                        serde_json::to_string(&fallback.on_timeout)?
                            .trim_matches('"')
                            .to_string(),
                    ));
                }
                if let Some(poll) = &describe.poll_strategy {
                    rows.push(("poll_strategy", poll.to_string()));
                }
                rows.push(("artifacts", describe.artifact_types.join(", ")));
                if !describe.optional_env_vars.is_empty() {
                    let names: Vec<&str> = describe
                        .optional_env_vars
                        .iter()
                        .map(|env| env.name.as_str())
                        .collect();
                    rows.push(("optional_env", names.join(", ")));
                }
            }
            print_table(&rows);
        }
    }
    Ok(())
}
