use anyhow::{Context, Result};

use brrmmmm::controller::{inspect_wasm_contract, validate_inspection};

use crate::cli::OutputFormat;

use super::output::print_table;

pub(crate) fn cmd_validate(wasm_path: &str, output: OutputFormat) -> Result<()> {
    let inspection = inspect_wasm_contract(wasm_path)
        .with_context(|| format!("WASM module failed to compile/validate: {wasm_path}"))?;
    validate_inspection(&inspection)?;

    match output {
        OutputFormat::Text => {
            eprintln!("[brrmmmm] validating {wasm_path}");
            eprintln!("[brrmmmm] ✓ WASM module validates successfully");
            eprintln!(
                "[brrmmmm]   entry: {}",
                inspection.entrypoint.as_deref().unwrap_or("unknown")
            );
            eprintln!("[brrmmmm]   ABI: v{}", inspection.abi_version);
            eprintln!("[brrmmmm]   size: {} bytes", inspection.wasm_size_bytes);
            if let Some(describe) = &inspection.describe {
                eprintln!(
                    "[brrmmmm]   contract: {} ({})",
                    describe.name, describe.logical_id
                );
                if !describe.run_modes.is_empty() {
                    eprintln!("[brrmmmm]   modes: {}", describe.run_modes.join(", "));
                }
            }
            if !inspection.brrmmmm_exports.is_empty() {
                eprintln!(
                    "[brrmmmm]   exports: {}",
                    inspection.brrmmmm_exports.join(", ")
                );
            }
        }
        OutputFormat::Json => {
            let describe = inspection.describe.as_ref();
            let obj = serde_json::json!({
                "valid": true,
                "abi_version": inspection.abi_version,
                "size_bytes": inspection.wasm_size_bytes,
                "entrypoint": inspection.entrypoint,
                "name": describe.map(|value| &value.name),
                "logical_id": describe.map(|value| &value.logical_id),
                "modes": describe.map(|value| &value.run_modes),
                "exports": inspection.brrmmmm_exports,
            });
            println!("{}", serde_json::to_string_pretty(&obj)?);
        }
        OutputFormat::Table => {
            let describe = inspection.describe.as_ref();
            let mut rows: Vec<(&str, String)> = vec![
                ("valid", "✓".to_string()),
                ("abi_version", inspection.abi_version.to_string()),
                ("size_bytes", inspection.wasm_size_bytes.to_string()),
                (
                    "entrypoint",
                    inspection.entrypoint.clone().unwrap_or_default(),
                ),
            ];
            if let Some(describe) = describe {
                rows.push(("name", describe.name.clone()));
                rows.push(("logical_id", describe.logical_id.clone()));
                if !describe.run_modes.is_empty() {
                    rows.push(("modes", describe.run_modes.join(", ")));
                }
            }
            if !inspection.brrmmmm_exports.is_empty() {
                rows.push(("exports", inspection.brrmmmm_exports.join(", ")));
            }
            print_table(&rows);
        }
    }

    Ok(())
}
