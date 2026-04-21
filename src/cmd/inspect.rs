use anyhow::Result;
use std::path::Path;

use brrmmmm::abi::MissionModuleDescribe;
use brrmmmm::config::Config;
use brrmmmm::controller::{MissionInspection, inspect_module_contract};

use crate::cli::OutputFormat;

use super::output::print_table;

pub fn cmd_inspect(wasm_path: &Path, output: OutputFormat, config: &Config) -> Result<()> {
    let wasm_str = wasm_path.to_string_lossy();
    let inspection = inspect_module_contract(&wasm_str)?;

    match output {
        OutputFormat::Json => print_inspect_json(&inspection, config)?,
        OutputFormat::Text => print_inspect_text(&inspection, &wasm_str, config)?,
        OutputFormat::Table => print_inspect_table(&inspection, config)?,
    }
    Ok(())
}

fn print_inspect_json(inspection: &MissionInspection, config: &Config) -> Result<()> {
    let mut value = serde_json::to_value(inspection)?;
    value["assurance_defaults"] = serde_json::json!({
        "same_reason_retry_limit": config.assurance.same_reason_retry_limit,
        "default_retry_after_ms": config.assurance.default_retry_after_ms,
    });
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

fn print_inspect_text(
    inspection: &MissionInspection,
    wasm_str: &str,
    config: &Config,
) -> Result<()> {
    let describe = inspection.describe.as_ref();
    eprintln!("[brrmmmm] inspecting {wasm_str}");
    println!(
        "logical_id:     {}",
        describe.map_or("-", |value| value.logical_id.as_str())
    );
    println!(
        "name:           {}",
        describe.map_or("-", |value| value.name.as_str())
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
    print_describe_text(describe)?;
    Ok(())
}

fn print_describe_text(describe: Option<&MissionModuleDescribe>) -> Result<()> {
    let Some(describe) = describe else {
        return Ok(());
    };
    let persistence = serde_json::to_string(&describe.state_persistence)?;
    let persistence = persistence.trim_matches('"');
    println!("persistence:   {persistence}");
    println!(
        "acq_timeout:   {}",
        describe
            .acquisition_timeout_secs
            .map_or_else(|| "-".to_string(), |secs| format!("{secs}s"))
    );
    if let Some(fallback) = &describe.operator_fallback {
        let timeout_outcome = serde_json::to_string(&fallback.on_timeout)?;
        let timeout_outcome = timeout_outcome.trim_matches('"');
        println!("operator_ttl:  {} ms", fallback.timeout_ms);
        println!("operator_on:   {timeout_outcome}");
    }
    if let Some(poll) = &describe.poll_strategy {
        println!("poll_strategy:  {poll}");
    }
    println!("artifacts:      {}", describe.artifact_types.join(", "));
    if !describe.optional_env_vars.is_empty() {
        let names = describe
            .optional_env_vars
            .iter()
            .map(|env| env.name.as_str())
            .collect::<Vec<_>>();
        println!("optional_env:   {}", names.join(", "));
    }
    Ok(())
}

fn print_inspect_table(inspection: &MissionInspection, config: &Config) -> Result<()> {
    let rows = inspection_rows(inspection, config)?;
    print_table(&rows);
    Ok(())
}

fn inspection_rows<'a>(
    inspection: &'a MissionInspection,
    config: &Config,
) -> Result<Vec<(&'a str, String)>> {
    let describe = inspection.describe.as_ref();
    let mut rows = vec![
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
                config.assurance.same_reason_retry_limit, config.assurance.default_retry_after_ms
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
    extend_describe_rows(&mut rows, describe)?;
    Ok(rows)
}

fn extend_describe_rows(
    rows: &mut Vec<(&str, String)>,
    describe: Option<&MissionModuleDescribe>,
) -> Result<()> {
    let Some(describe) = describe else {
        return Ok(());
    };
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
        let names = describe
            .optional_env_vars
            .iter()
            .map(|env| env.name.as_str())
            .collect::<Vec<_>>();
        rows.push(("optional_env", names.join(", ")));
    }
    Ok(())
}
