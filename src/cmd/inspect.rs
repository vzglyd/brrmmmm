use anyhow::Result;
use std::path::Path;

use brrmmmm::controller::inspect_module_contract;

use crate::cli::OutputFormat;

use super::output::print_table;

pub(crate) fn cmd_inspect(wasm_path: &Path, output: OutputFormat) -> Result<()> {
    let wasm_str = wasm_path.to_string_lossy();
    let inspection = inspect_module_contract(&wasm_str)?;

    match output {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&inspection)?);
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
