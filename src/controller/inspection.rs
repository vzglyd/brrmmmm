use std::sync::{Arc, Mutex, atomic::AtomicBool};

use anyhow::{Context, Result};
use serde::Serialize;
use wasmtime::{Engine, Linker, Module, Store};

use crate::abi::{
    ABI_VERSION_V1, ABI_VERSION_V2, ActiveMode, SidecarDescribe, SidecarRuntimeState,
};
use crate::events::EventSink;
use crate::host::HostState;

use super::host_imports::register_vzglyd_host_on_linker;

// ── Inspection ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct SidecarInspection {
    pub wasm_path: String,
    pub wasm_size_bytes: usize,
    pub abi_version: u32,
    pub active_mode: ActiveMode,
    pub entrypoint: Option<String>,
    pub brrmmmm_exports: Vec<String>,
    pub describe: Option<SidecarDescribe>,
    pub diagnostics: Vec<String>,
}

// ── ABI version detection ────────────────────────────────────────────

pub fn detect_abi_version(module: &Module) -> u32 {
    let has_v2 = module
        .exports()
        .any(|e| e.name() == "vzglyd_sidecar_abi_version");
    if has_v2 {
        ABI_VERSION_V2
    } else {
        ABI_VERSION_V1
    }
}

pub fn inspect_wasm_contract(wasm_path: &str) -> Result<SidecarInspection> {
    let wasm_bytes =
        std::fs::read(wasm_path).with_context(|| format!("read WASM file: {wasm_path}"))?;

    let engine = Engine::default();
    let module = Module::from_binary(&engine, &wasm_bytes)
        .with_context(|| format!("compile WASM module: {wasm_path}"))?;
    let abi_version = detect_abi_version(&module);
    let active_mode = match abi_version {
        ABI_VERSION_V2 => ActiveMode::ManagedPolling,
        _ => ActiveMode::V1Legacy,
    };
    let entrypoint = find_entry_export(&module, abi_version);
    let brrmmmm_exports = brrmmmm_exports(&module);
    let mut diagnostics = Vec::new();

    let describe = if abi_version == ABI_VERSION_V2 {
        let (mut store, instance) = instantiate_for_inspection(&engine, &module)?;
        let exported_abi = call_exported_abi_version(&instance, &mut store)?;
        if exported_abi != ABI_VERSION_V2 {
            anyhow::bail!(
                "unsupported sidecar ABI version {exported_abi}; supported ABI versions are {ABI_VERSION_V1} and {ABI_VERSION_V2}"
            );
        }

        match read_static_describe(&instance, &mut store)? {
            Some(describe) => Some(describe),
            None => {
                diagnostics.push(
                    "v2 sidecar is missing vzglyd_sidecar_describe_ptr/len exports".to_string(),
                );
                None
            }
        }
    } else {
        diagnostics.push(
            "v1 sidecar has no static self-description; behavior is inferred at runtime"
                .to_string(),
        );
        None
    };

    Ok(SidecarInspection {
        wasm_path: wasm_path.to_string(),
        wasm_size_bytes: wasm_bytes.len(),
        abi_version,
        active_mode,
        entrypoint,
        brrmmmm_exports,
        describe,
        diagnostics,
    })
}

pub fn validate_inspection(inspection: &SidecarInspection) -> Result<()> {
    if inspection.entrypoint.is_none() {
        anyhow::bail!("WASM module has no recognised entry point");
    }

    if inspection.abi_version == ABI_VERSION_V2 {
        let describe = inspection
            .describe
            .as_ref()
            .context("v2 sidecar must export a valid static describe contract")?;
        validate_describe_contract(describe)?;
    }

    Ok(())
}

fn validate_describe_contract(describe: &SidecarDescribe) -> Result<()> {
    if describe.schema_version == 0 {
        anyhow::bail!("describe.schema_version must be greater than zero");
    }
    if describe.logical_id.trim().is_empty() {
        anyhow::bail!("describe.logical_id is required");
    }
    if describe.name.trim().is_empty() {
        anyhow::bail!("describe.name is required");
    }
    if describe.abi_version != 0 && describe.abi_version != ABI_VERSION_V2 {
        anyhow::bail!(
            "describe.abi_version must be {ABI_VERSION_V2} when present, got {}",
            describe.abi_version
        );
    }
    for mode in &describe.run_modes {
        match mode.as_str() {
            "v1_legacy" | "managed_polling" | "interactive" => {}
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

fn instantiate_for_inspection(
    engine: &Engine,
    module: &Module,
) -> Result<(
    Store<wasmtime_wasi::preview1::WasiP1Ctx>,
    wasmtime::Instance,
)> {
    let wasi_p1 = wasmtime_wasi::WasiCtxBuilder::new().build_p1();
    let mut store = Store::new(engine, wasi_p1);
    let mut linker: Linker<wasmtime_wasi::preview1::WasiP1Ctx> = Linker::new(engine);
    wasmtime_wasi::preview1::add_to_linker_sync(&mut linker, |ctx| ctx)?;

    let runtime_state = Arc::new(Mutex::new(SidecarRuntimeState::default()));
    register_vzglyd_host_on_linker(
        &mut linker,
        HostState::new(false, Arc::new(Mutex::new(None))),
        EventSink::noop(),
        runtime_state,
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
    )?;

    let instance = linker
        .instantiate(&mut store, module)
        .context("instantiate WASM module for inspection")?;
    Ok((store, instance))
}

fn call_exported_abi_version(
    instance: &wasmtime::Instance,
    store: &mut Store<wasmtime_wasi::preview1::WasiP1Ctx>,
) -> Result<u32> {
    let abi_fn = instance
        .get_typed_func::<(), u32>(&mut *store, "vzglyd_sidecar_abi_version")
        .context("v2 sidecar must export callable vzglyd_sidecar_abi_version() -> u32")?;
    abi_fn
        .call(store, ())
        .context("call vzglyd_sidecar_abi_version")
}

pub(super) fn read_static_describe(
    instance: &wasmtime::Instance,
    store: &mut Store<wasmtime_wasi::preview1::WasiP1Ctx>,
) -> Result<Option<SidecarDescribe>> {
    let ptr_fn = instance
        .get_typed_func::<(), i32>(&mut *store, "vzglyd_sidecar_describe_ptr")
        .ok();
    let len_fn = instance
        .get_typed_func::<(), i32>(&mut *store, "vzglyd_sidecar_describe_len")
        .ok();
    let (Some(ptr_fn), Some(len_fn)) = (ptr_fn, len_fn) else {
        return Ok(None);
    };

    let ptr = ptr_fn
        .call(&mut *store, ())
        .context("call vzglyd_sidecar_describe_ptr")?;
    let len = len_fn
        .call(&mut *store, ())
        .context("call vzglyd_sidecar_describe_len")?;
    if ptr < 0 || len <= 0 {
        anyhow::bail!("invalid describe memory range: ptr={ptr}, len={len}");
    }

    let memory = instance
        .get_memory(&mut *store, "memory")
        .context("sidecar describe requires exported memory")?;
    let mut bytes = vec![0; len as usize];
    memory
        .read(&mut *store, ptr as usize, &mut bytes)
        .context("read sidecar describe bytes")?;
    let describe = serde_json::from_slice::<SidecarDescribe>(&bytes)
        .context("decode sidecar describe JSON")?;
    Ok(Some(describe))
}

fn brrmmmm_exports(module: &Module) -> Vec<String> {
    module
        .exports()
        .filter(|e| {
            let n = e.name();
            n.starts_with("vzglyd_") || n == "_start" || n == "main"
        })
        .map(|e| e.name().to_string())
        .collect()
}

fn find_entry_export(module: &Module, abi_version: u32) -> Option<String> {
    let names: &[&str] = if abi_version == ABI_VERSION_V2 {
        &["vzglyd_sidecar_start", "_start", "main"]
    } else {
        &["_start", "main"]
    };
    names
        .iter()
        .find(|name| module.get_export(name).is_some())
        .map(|name| (*name).to_string())
}
