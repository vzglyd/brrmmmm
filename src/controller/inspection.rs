//! WASM mission-module inspection and contract validation.

use std::sync::{Arc, Mutex, atomic::AtomicBool};

use anyhow::{Context, Result};
use serde::Serialize;
use wasmtime::{Config, Engine, Module};

use crate::abi::{ABI_VERSION_V4, ActiveMode, MissionModuleDescribe, MissionRuntimeState};
use crate::events::EventSink;
use crate::host::HostState;

use super::host_imports::register_brrmmmm_host_on_linker;
use super::io::{RuntimePolicy, WasmLinker, WasmStore, build_wasm_store};

// ── Inspection ───────────────────────────────────────────────────────

/// Summary of the static contract discovered in a mission-module WASM module.
#[derive(Debug, Clone, Serialize)]
pub struct MissionInspection {
    /// Path to the inspected WASM module.
    pub wasm_path: String,
    /// WASM file size in bytes.
    pub wasm_size_bytes: usize,
    /// ABI version exported by the mission module.
    pub abi_version: u32,
    /// Runtime mode inferred for inspection output.
    pub active_mode: ActiveMode,
    /// Recognized entrypoint export, when one exists.
    pub entrypoint: Option<String>,
    /// Export names in the `brrmmmm_*` namespace.
    pub brrmmmm_exports: Vec<String>,
    /// Imported names from the `brrmmmm_host` namespace.
    pub host_imports: Vec<String>,
    /// Static describe contract when the mission module exports one.
    pub describe: Option<MissionModuleDescribe>,
    /// Non-fatal inspection warnings and omissions.
    pub diagnostics: Vec<String>,
}

/// Inspect a mission-module WASM module without executing an acquisition mission.
///
/// # Errors
///
/// Returns an error when the WASM cannot be read, instantiated, or does not
/// satisfy the required ABI surface for inspection.
pub fn inspect_module_contract(wasm_path: &str) -> Result<MissionInspection> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build tokio runtime for inspection")?;
    runtime.block_on(inspect_module_contract_async(wasm_path))
}

async fn inspect_module_contract_async(wasm_path: &str) -> Result<MissionInspection> {
    let wasm_bytes =
        std::fs::read(wasm_path).with_context(|| format!("read WASM file: {wasm_path}"))?;

    let mut engine_config = Config::new();
    engine_config.epoch_interruption(true);
    engine_config.async_support(true);
    let engine = Engine::new(&engine_config).context("create wasmtime engine")?;
    let module = Module::from_binary(&engine, &wasm_bytes)
        .with_context(|| format!("compile WASM module: {wasm_path}"))?;

    let has_abi_export = module
        .exports()
        .any(|e| e.name() == "brrmmmm_module_abi_version");
    if !has_abi_export {
        anyhow::bail!(
            "WASM module does not export brrmmmm_module_abi_version; supported ABI version is {ABI_VERSION_V4}"
        );
    }

    let (mut store, instance) = instantiate_for_inspection(&engine, &module).await?;
    let exported_abi = call_exported_abi_version(&instance, &mut store).await?;
    if exported_abi != ABI_VERSION_V4 {
        anyhow::bail!(
            "unsupported mission-module ABI version {exported_abi}; supported ABI version is {ABI_VERSION_V4}"
        );
    }

    let entrypoint = find_entry_export(&module);
    let brrmmmm_exports = brrmmmm_exports(&module);
    let host_imports = brrmmmm_host_imports(&module);
    let mut diagnostics = Vec::new();

    let describe = read_static_describe(&instance, &mut store, &RuntimePolicy::default())
        .await?
        .map_or_else(
            || {
                diagnostics.push(
                    "mission module is missing brrmmmm_module_describe_ptr/len exports".to_string(),
                );
                None
            },
            Some,
        );
    if !host_imports
        .iter()
        .any(|name| name == "mission_outcome_report")
    {
        diagnostics.push(
            "mission module is missing required brrmmmm_host.mission_outcome_report import"
                .to_string(),
        );
    }

    Ok(MissionInspection {
        wasm_path: wasm_path.to_string(),
        wasm_size_bytes: wasm_bytes.len(),
        abi_version: exported_abi,
        active_mode: ActiveMode::ManagedPolling,
        entrypoint,
        brrmmmm_exports,
        host_imports,
        describe,
        diagnostics,
    })
}

/// Validate that an inspection result satisfies the minimum runnable contract.
///
/// # Errors
///
/// Returns an error when the inspection result is missing required entrypoints,
/// host imports, or describe metadata.
pub fn validate_module_inspection(inspection: &MissionInspection) -> Result<()> {
    if inspection.entrypoint.is_none() {
        anyhow::bail!("WASM module must export brrmmmm_module_start");
    }

    let describe = inspection
        .describe
        .as_ref()
        .context("mission module must export a valid static describe contract")?;
    validate_describe_contract(describe)?;
    if !inspection
        .host_imports
        .iter()
        .any(|name| name == "mission_outcome_report")
    {
        anyhow::bail!("mission module must import brrmmmm_host.mission_outcome_report");
    }

    Ok(())
}

fn validate_describe_contract(describe: &MissionModuleDescribe) -> Result<()> {
    if describe.schema_version == 0 {
        anyhow::bail!("describe.schema_version must be greater than zero");
    }
    if describe.logical_id.trim().is_empty() {
        anyhow::bail!("describe.logical_id is required");
    }
    if describe.name.trim().is_empty() {
        anyhow::bail!("describe.name is required");
    }
    if describe.description.trim().is_empty() {
        anyhow::bail!("describe.description is required");
    }
    if describe.abi_version != 0 && describe.abi_version != ABI_VERSION_V4 {
        anyhow::bail!(
            "describe.abi_version must be {ABI_VERSION_V4} when present, got {}",
            describe.abi_version
        );
    }
    if describe.acquisition_timeout_secs == Some(0) {
        anyhow::bail!("describe.acquisition_timeout_secs must be greater than zero when present");
    }
    if describe
        .operator_fallback
        .as_ref()
        .is_some_and(|fallback| fallback.timeout_ms == 0)
    {
        anyhow::bail!("describe.operator_fallback.timeout_ms must be greater than zero");
    }
    for mode in &describe.run_modes {
        match mode.as_str() {
            "managed_polling" | "interactive" => {}
            _ => anyhow::bail!("unknown run mode in describe.run_modes: {mode}"),
        }
    }
    if describe.artifact_types.is_empty()
        || !describe
            .artifact_types
            .iter()
            .any(|kind| kind == "published_output")
    {
        anyhow::bail!("describe.artifact_types must include published_output");
    }
    Ok(())
}

async fn instantiate_for_inspection(
    engine: &Engine,
    module: &Module,
) -> Result<(WasmStore, wasmtime::Instance)> {
    let policy = RuntimePolicy::default();
    let wasi_p1 = wasmtime_wasi::WasiCtxBuilder::new().build_p1();
    let mut store = build_wasm_store(engine, wasi_p1, &policy);
    store.set_epoch_deadline(policy.init_deadline_ticks());
    store.epoch_deadline_trap();
    let mut linker = WasmLinker::new(engine);
    wasmtime_wasi::preview1::add_to_linker_async(&mut linker, |state| &mut state.wasi)?;

    let runtime_state = Arc::new(Mutex::new(MissionRuntimeState::default()));
    let config = crate::config::Config::load().context("load brrmmmm config for inspection")?;
    let _shared_host_state = register_brrmmmm_host_on_linker(
        &mut linker,
        HostState::new(
            false,
            Arc::new(Mutex::new(None)),
            crate::identity::ModuleHash([0u8; 32]),
            None,
            config.clone(),
        ),
        EventSink::noop(),
        runtime_state,
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
        None,
        &config,
    )?;

    let instance = linker
        .instantiate_async(&mut store, module)
        .await
        .context("instantiate WASM module for inspection")?;
    Ok((store, instance))
}

async fn call_exported_abi_version(
    instance: &wasmtime::Instance,
    store: &mut WasmStore,
) -> Result<u32> {
    let abi_fn = instance
        .get_typed_func::<(), u32>(&mut *store, "brrmmmm_module_abi_version")
        .context("mission module must export callable brrmmmm_module_abi_version() -> u32")?;
    abi_fn
        .call_async(store, ())
        .await
        .context("call brrmmmm_module_abi_version")
}

pub(super) async fn read_static_describe(
    instance: &wasmtime::Instance,
    store: &mut WasmStore,
    policy: &RuntimePolicy,
) -> Result<Option<MissionModuleDescribe>> {
    let ptr_fn = instance
        .get_typed_func::<(), i32>(&mut *store, "brrmmmm_module_describe_ptr")
        .ok();
    let len_fn = instance
        .get_typed_func::<(), i32>(&mut *store, "brrmmmm_module_describe_len")
        .ok();
    let (Some(ptr_fn), Some(len_fn)) = (ptr_fn, len_fn) else {
        return Ok(None);
    };

    let ptr = ptr_fn
        .call_async(&mut *store, ())
        .await
        .context("call brrmmmm_module_describe_ptr")?;
    let len = len_fn
        .call_async(&mut *store, ())
        .await
        .context("call brrmmmm_module_describe_len")?;
    if ptr < 0 || len <= 0 {
        anyhow::bail!("invalid describe memory range: ptr={ptr}, len={len}");
    }
    let ptr = usize::try_from(ptr).context("mission describe pointer must be non-negative")?;
    let len = usize::try_from(len).context("mission describe length must be positive")?;
    if len > policy.max_describe_bytes {
        anyhow::bail!(
            "mission describe is {len} bytes, exceeding the configured limit of {} bytes",
            policy.max_describe_bytes
        );
    }

    let memory = instance
        .get_memory(&mut *store, "memory")
        .context("mission describe requires exported memory")?;
    let mut bytes = vec![0; len];
    memory
        .read(&mut *store, ptr, &mut bytes)
        .context("read mission describe bytes")?;
    let describe = serde_json::from_slice::<MissionModuleDescribe>(&bytes)
        .context("decode mission describe JSON")?;
    Ok(Some(describe))
}

fn brrmmmm_exports(module: &Module) -> Vec<String> {
    module
        .exports()
        .filter(|e| e.name().starts_with("brrmmmm_"))
        .map(|e| e.name().to_string())
        .collect()
}

fn brrmmmm_host_imports(module: &Module) -> Vec<String> {
    module
        .imports()
        .filter(|import| import.module() == "brrmmmm_host")
        .map(|import| import.name().to_string())
        .collect()
}

fn find_entry_export(module: &Module) -> Option<String> {
    module
        .get_export("brrmmmm_module_start")
        .map(|_| "brrmmmm_module_start".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::abi::{OperatorFallbackPolicy, OperatorTimeoutOutcome, PersistenceAuthority};

    fn valid_inspection() -> MissionInspection {
        MissionInspection {
            wasm_path: "test.wasm".to_string(),
            wasm_size_bytes: 1,
            abi_version: ABI_VERSION_V4,
            active_mode: ActiveMode::ManagedPolling,
            entrypoint: Some("brrmmmm_module_start".to_string()),
            brrmmmm_exports: vec!["brrmmmm_module_start".to_string()],
            host_imports: vec!["mission_outcome_report".to_string()],
            describe: Some(MissionModuleDescribe {
                schema_version: 1,
                logical_id: "brrmmmm.test".to_string(),
                name: "Test Mission Module".to_string(),
                description: "Test mission module".to_string(),
                abi_version: ABI_VERSION_V4,
                run_modes: vec!["managed_polling".to_string()],
                state_persistence: PersistenceAuthority::Volatile,
                required_env_vars: vec![],
                optional_env_vars: vec![],
                params: None,
                capabilities_needed: vec![],
                poll_strategy: None,
                cooldown_policy: None,
                artifact_types: vec!["published_output".to_string()],
                acquisition_timeout_secs: Some(30),
                operator_fallback: None,
            }),
            diagnostics: vec![],
        }
    }

    #[test]
    fn validate_rejects_zero_operator_fallback_timeout() {
        let mut inspection = valid_inspection();
        inspection.describe.as_mut().unwrap().operator_fallback = Some(OperatorFallbackPolicy {
            timeout_ms: 0,
            on_timeout: OperatorTimeoutOutcome::RetryableFailure,
        });

        let error = validate_module_inspection(&inspection).expect_err("validation must fail");
        assert!(
            error
                .to_string()
                .contains("describe.operator_fallback.timeout_ms must be greater than zero")
        );
    }
}
