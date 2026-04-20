use std::sync::{
    Arc, Mutex,
    atomic::AtomicBool,
};

use anyhow::{Context, Result};
use wasmtime::{Engine, Linker, Module, Store};

use crate::abi::{PersistenceAuthority, SidecarRuntimeState};
use crate::events::{Event, EventSink, diag, now_ts};
use crate::host::{ArtifactStore, HostState};
use crate::identity::InstallationIdentity;
use crate::persistence;

use super::host_imports::register_vzglyd_host_on_linker;
use super::inspection::read_static_describe;
use super::io::{lock_runtime, update_failure_state};

// ── WASM instance runner ─────────────────────────────────────────────

pub(super) struct WasmRunConfig {
    pub(super) wasm_path: String,
    pub(super) env_vars: Vec<(String, String)>,
    pub(super) params_bytes: Option<Vec<u8>>,
    pub(super) log_channel: bool,
    pub(super) abi_version: u32,
    pub(super) wasm_size_bytes: usize,
    pub(super) wasm_hash: String,
    pub(super) module_hash: [u8; 32],
    pub(super) attestation_identity: Option<InstallationIdentity>,
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
        wasm_hash,
        module_hash,
        attestation_identity,
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
    // Set a generous initial epoch deadline covering module initialization and describe().
    // With epoch_interruption enabled, the default deadline is 0 (immediately interruptible).
    // This will be reset to the actual acquisition timeout after describe() is read.
    store.set_epoch_deadline(600); // 60 s at 10 Hz, covers init/describe
    store.epoch_deadline_trap();
    let mut linker: Linker<wasmtime_wasi::preview1::WasiP1Ctx> = Linker::new(engine);
    wasmtime_wasi::preview1::add_to_linker_sync(&mut linker, |ctx| ctx)?;

    // Build host state and register all vzglyd_host imports.
    let mut host_state =
        HostState::new(log_channel, params_state, module_hash, attestation_identity);
    // Share the artifact_store from the controller.
    host_state.artifact_store = artifact_store;

    let shared_host_state = register_vzglyd_host_on_linker(
        &mut linker,
        host_state,
        event_sink.clone(),
        runtime_state.clone(),
        stop_signal.clone(),
        force_refresh,
        Some(wasm_hash.clone()),
    )?;

    let instance = linker
        .instantiate(&mut store, module)
        .context("instantiate WASM module")?;

    configure_sidecar_params(&instance, &mut store, params_bytes.as_deref(), &event_sink)?;

    // Emit Started event (before calling the entry point).
    event_sink.emit(Event::Started {
        ts: now_ts(),
        wasm_path: wasm_path.clone(),
        wasm_size_bytes,
        abi_version,
    });

    match call_describe(&instance, &mut store) {
        Ok(Some(describe)) => {
            let timeout_secs = describe
                .acquisition_timeout_secs
                .filter(|timeout_secs| *timeout_secs > 0)
                .unwrap_or(30) as u64;
            event_sink.emit(Event::Describe {
                ts: now_ts(),
                describe: describe.clone(),
            });
            {
                let mut host = lock_runtime(&shared_host_state, "host_state");
                host.set_mission_describe(&describe);
            }
            lock_runtime(&runtime_state, "runtime_state").describe = Some(describe);

            // Set hard epoch deadline: epoch timer increments at ~10 Hz (100ms intervals),
            // so timeout_secs * 10 ticks gives the acquisition budget as a hard cutoff
            // that fires even if the guest is in a tight compute loop.
            store.set_epoch_deadline(timeout_secs * 10);
            store.epoch_deadline_trap();
            diag(
                &event_sink,
                &format!(
                    "[brrmmmm] acquisition budget of {timeout_secs}s enforced via epoch interrupt"
                ),
            );
        }
        Ok(None) => diag(
            &event_sink,
            "[brrmmmm] sidecar is missing static describe exports",
        ),
        Err(error) => {
            update_failure_state(&runtime_state, &error.to_string());
            return Err(error);
        }
    }

    diag(&event_sink, "[brrmmmm] starting sidecar...");

    let entry_name = find_entry(&instance, &mut store);

    let entry_name = entry_name.context("WASM module has no recognised entry point")?;
    let entry = instance
        .get_func(&mut store, &entry_name)
        .with_context(|| format!("get entry function: {entry_name}"))?;

    diag(
        &event_sink,
        &format!("[brrmmmm] calling entry '{entry_name}' (runs until stopped)"),
    );

    let call_result = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        entry.call(&mut store, &[], &mut [])
    })) {
        Ok(result) => result,
        Err(panic_payload) => {
            let msg = panic_payload
                .downcast_ref::<String>()
                .map(|s| s.as_str())
                .or_else(|| panic_payload.downcast_ref::<&str>().copied())
                .unwrap_or("unknown panic");
            diag(&event_sink, &format!("[brrmmmm] host import panicked: {msg}"));
            Err(anyhow::anyhow!("host import panic: {msg}"))
        }
    };

    let reason = match &call_result {
        Ok(_) => "completed",
        Err(e) if e.to_string().contains("interrupt") => "timeout",
        Err(_) => "error",
    };
    event_sink.emit(Event::SidecarExit {
        ts: now_ts(),
        reason: reason.to_string(),
    });

    // Save persisted state if the sidecar requests it.
    {
        let state = lock_runtime(&runtime_state, "runtime_state");
        let should_persist = state
            .describe
            .as_ref()
            .map(|d| d.state_persistence == PersistenceAuthority::HostPersisted)
            .unwrap_or(false);

        if should_persist {
            if let Err(error) = persistence::save(&wasm_hash, &state) {
                diag(
                    &event_sink,
                    &format!("[brrmmmm] failed to persist runtime state: {error:#}"),
                );
            }
        }
    }

    call_result
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("{e:?}"))
}

// ── Entry point resolution ───────────────────────────────────────────

fn find_entry(
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

// ── describe() call ──────────────────────────────────────────────────

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

    if params.len() > capacity {
        diag(
            event_sink,
            &format!(
                "[brrmmmm] sidecar params ({} bytes) exceed buffer capacity ({} bytes); aborting param write",
                params.len(),
                capacity
            ),
        );
        return Err(anyhow::anyhow!(
            "params ({} bytes) exceed sidecar buffer capacity ({capacity} bytes)",
            params.len()
        ));
    }

    let memory = instance
        .get_memory(&mut *store, "memory")
        .context("sidecar configure requires exported memory")?;
    memory
        .write(&mut *store, ptr, params)
        .context("write sidecar params")?;
    let status = cfg_fn
        .call(&mut *store, params.len() as i32)
        .context("call vzglyd_configure")?;
    diag(
        event_sink,
        &format!("[brrmmmm] sidecar vzglyd_configure({}) -> {status}", params.len()),
    );
    Ok(())
}
