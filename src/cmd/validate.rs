use anyhow::{Context, Result};
use std::path::Path;

use brrmmmm::controller::{inspect_module_contract, validate_module_inspection};

use crate::cli::OutputFormat;

use super::output::print_table;

pub(crate) fn cmd_validate(wasm_path: &Path, output: OutputFormat) -> Result<()> {
    let wasm_str = wasm_path.to_string_lossy();
    let inspection = inspect_module_contract(&wasm_str)
        .with_context(|| format!("WASM module failed to compile/validate: {wasm_str}"))?;
    validate_module_inspection(&inspection)?;

    match output {
        OutputFormat::Text => {
            eprintln!("[brrmmmm] validating {wasm_str}");
            eprintln!("[brrmmmm] ✓ WASM module validates successfully");
            eprintln!(
                "[brrmmmm]   entry: {}",
                inspection.entrypoint.as_deref().unwrap_or("unknown")
            );
            eprintln!("[brrmmmm]   ABI: v{}", inspection.abi_version);
            eprintln!("[brrmmmm]   size: {} bytes", inspection.wasm_size_bytes);
            if let Some(describe) = &inspection.describe {
                let persistence = serde_json::to_string(&describe.state_persistence)?;
                let persistence = persistence.trim_matches('"');
                eprintln!(
                    "[brrmmmm]   contract: {} ({})",
                    describe.name, describe.logical_id
                );
                if !describe.run_modes.is_empty() {
                    eprintln!("[brrmmmm]   modes: {}", describe.run_modes.join(", "));
                }
                eprintln!("[brrmmmm]   persistence: {}", persistence);
                if let Some(timeout_secs) = describe.acquisition_timeout_secs {
                    eprintln!("[brrmmmm]   acquisition timeout: {timeout_secs}s");
                }
                if let Some(fallback) = &describe.operator_fallback {
                    let timeout_outcome = serde_json::to_string(&fallback.on_timeout)?;
                    let timeout_outcome = timeout_outcome.trim_matches('"');
                    eprintln!("[brrmmmm]   operator timeout: {} ms", fallback.timeout_ms);
                    eprintln!("[brrmmmm]   operator timeout outcome: {}", timeout_outcome);
                }
            }
            if !inspection.brrmmmm_exports.is_empty() {
                eprintln!(
                    "[brrmmmm]   exports: {}",
                    inspection.brrmmmm_exports.join(", ")
                );
            }
            if !inspection.host_imports.is_empty() {
                eprintln!(
                    "[brrmmmm]   host imports: {}",
                    inspection.host_imports.join(", ")
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
                "persistence": describe.and_then(|value| serde_json::to_value(&value.state_persistence).ok()),
                "acquisition_timeout_secs": describe.and_then(|value| value.acquisition_timeout_secs),
                "operator_fallback": describe.and_then(|value| value.operator_fallback.as_ref()),
                "exports": inspection.brrmmmm_exports,
                "host_imports": inspection.host_imports,
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
                rows.push((
                    "persistence",
                    serde_json::to_string(&describe.state_persistence)?
                        .trim_matches('"')
                        .to_string(),
                ));
                if let Some(timeout_secs) = describe.acquisition_timeout_secs {
                    rows.push(("acq_timeout", format!("{timeout_secs}s")));
                }
                if let Some(fallback) = &describe.operator_fallback {
                    rows.push(("operator_ttl", format!("{} ms", fallback.timeout_ms)));
                    rows.push((
                        "operator_on",
                        serde_json::to_string(&fallback.on_timeout)?
                            .trim_matches('"')
                            .to_string(),
                    ));
                }
            }
            if !inspection.brrmmmm_exports.is_empty() {
                rows.push(("exports", inspection.brrmmmm_exports.join(", ")));
            }
            if !inspection.host_imports.is_empty() {
                rows.push(("host_imports", inspection.host_imports.join(", ")));
            }
            print_table(&rows);
        }
    }

    Ok(())
}
