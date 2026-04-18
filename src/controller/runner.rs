use std::sync::{Arc, Mutex, atomic::AtomicBool};

use anyhow::{Context, Result};
use wasmtime::{Engine, Linker, Module, Store};

use crate::abi::{ABI_VERSION_V2, SidecarRuntimeState};
use crate::events::{Event, EventSink, diag, now_ts};
use crate::host::{ArtifactStore, HostState};

use super::host_imports::register_vzglyd_host_on_linker;
use super::inspection::read_static_describe;
use super::io::update_failure_state;

// ── WASM instance runner ─────────────────────────────────────────────

pub(super) struct WasmRunConfig {
    pub(super) wasm_path: String,
    pub(super) env_vars: Vec<(String, String)>,
    pub(super) params_bytes: Option<Vec<u8>>,
    pub(super) log_channel: bool,
    pub(super) abi_version: u32,
    pub(super) wasm_size_bytes: usize,
}

pub(super) struct WasmRunContext {
    pub(super) artifact_store: Arc<Mutex<ArtifactStore>>,
    pub(super) runtime_state: Arc<Mutex<SidecarRuntimeState>>,
    pub(super) params_state: Arc<Mutex<Option<Vec<u8>>>>,
    pub(super) event_sink: EventSink,
    pub(super) stop_signal: Arc<AtomicBool>,
    pub(super) force_refresh: Arc<AtomicBool>,
}

pub(super) fn run_wasm_instance(
    engine: &Engine,
    module: &Module,
    config: WasmRunConfig,
    context: WasmRunContext,
) -> Result<()> {
    let WasmRunConfig {
        wasm_path,
        env_vars,
        params_bytes,
        log_channel,
        abi_version,
        wasm_size_bytes,
    } = config;
    let WasmRunContext {
        artifact_store,
        runtime_state,
        params_state,
        event_sink,
        stop_signal,
        force_refresh,
    } = context;

    // Build WASI preview1 context.
    let mut wasi_builder = wasmtime_wasi::WasiCtxBuilder::new();
    for (key, value) in &env_vars {
        let _ = wasi_builder.env(key, value);
    }
    wasi_builder.inherit_stdout().inherit_stderr();
    let wasi_p1 = wasi_builder.build_p1();

    let mut store = Store::new(engine, wasi_p1);
    let mut linker: Linker<wasmtime_wasi::preview1::WasiP1Ctx> = Linker::new(engine);
    wasmtime_wasi::preview1::add_to_linker_sync(&mut linker, |ctx| ctx)?;

    // Build host state and register all vzglyd_host imports.
    let mut host_state = HostState::new(log_channel, params_state);
    // Share the artifact_store from the controller.
    host_state.artifact_store = artifact_store;

    register_vzglyd_host_on_linker(
        &mut linker,
        host_state,
        event_sink.clone(),
        runtime_state.clone(),
        stop_signal,
        force_refresh,
    )?;

    let instance = linker
        .instantiate(&mut store, module)
        .context("instantiate WASM module")?;

    configure_sidecar_params(&instance, &mut store, params_bytes.as_deref(), &event_sink)?;

    // Emit Started event (before calling the entry point).
    event_sink.emit(Event::Started {
        ts: now_ts(),
        wasm_path,
        wasm_size_bytes,
        abi_version,
    });

    // For v2: read the static describe blob if available and update runtime_state.
    if abi_version == ABI_VERSION_V2 {
        match call_describe(&instance, &mut store) {
            Ok(Some(describe)) => {
                event_sink.emit(Event::Describe {
                    ts: now_ts(),
                    describe: describe.clone(),
                });
                runtime_state.lock().expect("mutex poisoned").describe = Some(describe);
            }
            Ok(None) => diag(
                &event_sink,
                "[brrmmmm] v2 sidecar is missing static describe exports",
            ),
            Err(error) => {
                update_failure_state(&runtime_state, &error.to_string());
                return Err(error);
            }
        }
    }

    diag(&event_sink, "[brrmmmm] starting sidecar...");

    // Find and call the entry point.
    // v2: prefer vzglyd_sidecar_start, then _start/main
    // v1: _start or main
    let entry_name = if abi_version == ABI_VERSION_V2 {
        find_entry_v2(&instance, &mut store)
    } else {
        find_entry_v1(&instance, &mut store)
    };

    let entry_name = entry_name.context("WASM module has no recognised entry point")?;
    let entry = instance
        .get_func(&mut store, &entry_name)
        .with_context(|| format!("get entry function: {entry_name}"))?;

    diag(
        &event_sink,
        &format!("[brrmmmm] calling entry '{entry_name}' (runs until stopped)"),
    );

    let call_result = entry.call(&mut store, &[], &mut []);

    let reason = match &call_result {
        Ok(_) => "completed",
        Err(_) => "error",
    };
    event_sink.emit(Event::SidecarExit {
        ts: now_ts(),
        reason: reason.to_string(),
    });

    call_result
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("{e:?}"))
}

// ── Entry point resolution ───────────────────────────────────────────

fn find_entry_v2(
    instance: &wasmtime::Instance,
    store: &mut Store<wasmtime_wasi::preview1::WasiP1Ctx>,
) -> Option<String> {
    for name in &["vzglyd_sidecar_start", "_start", "main"] {
        if instance.get_func(&mut *store, name).is_some() {
            return Some(name.to_string());
        }
    }
    None
}

fn find_entry_v1(
    instance: &wasmtime::Instance,
    store: &mut Store<wasmtime_wasi::preview1::WasiP1Ctx>,
) -> Option<String> {
    for name in &["_start", "main"] {
        if instance.get_func(&mut *store, name).is_some() {
            return Some(name.to_string());
        }
    }
    None
}

// ── v2 describe() call ───────────────────────────────────────────────

fn call_describe(
    instance: &wasmtime::Instance,
    store: &mut Store<wasmtime_wasi::preview1::WasiP1Ctx>,
) -> Result<Option<crate::abi::SidecarDescribe>> {
    read_static_describe(instance, store)
}

// ── Parameter configuration ──────────────────────────────────────────

fn configure_sidecar_params(
    instance: &wasmtime::Instance,
    store: &mut Store<wasmtime_wasi::preview1::WasiP1Ctx>,
    params_bytes: Option<&[u8]>,
    event_sink: &EventSink,
) -> Result<()> {
    let Some(params) = params_bytes else {
        return Ok(());
    };

    let ptr_fn = instance
        .get_typed_func::<(), i32>(&mut *store, "vzglyd_params_ptr")
        .ok();
    let cap_fn = instance
        .get_typed_func::<(), u32>(&mut *store, "vzglyd_params_capacity")
        .ok();
    let cfg_fn = instance
        .get_typed_func::<i32, i32>(&mut *store, "vzglyd_configure")
        .ok();
    let (Some(ptr_fn), Some(cap_fn), Some(cfg_fn)) = (ptr_fn, cap_fn, cfg_fn) else {
        diag(
            event_sink,
            "[brrmmmm] sidecar params provided, but configure exports are missing; params ignored",
        );
        return Ok(());
    };

    let capacity = cap_fn
        .call(&mut *store, ())
        .context("call vzglyd_params_capacity")? as usize;
    let ptr = ptr_fn
        .call(&mut *store, ())
        .context("call vzglyd_params_ptr")? as usize;
    let write_len = params.len().min(capacity);
    let memory = instance
        .get_memory(&mut *store, "memory")
        .context("sidecar configure requires exported memory")?;
    memory
        .write(&mut *store, ptr, &params[..write_len])
        .context("write sidecar params")?;
    let status = cfg_fn
        .call(&mut *store, write_len as i32)
        .context("call vzglyd_configure")?;
    diag(
        event_sink,
        &format!("[brrmmmm] sidecar vzglyd_configure({write_len}) -> {status}"),
    );
    Ok(())
}
