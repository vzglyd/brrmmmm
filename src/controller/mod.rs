use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::Serialize;
use wasmtime::{Engine, Linker, Module, Store};

use crate::abi::{
    ABI_VERSION_V1, ABI_VERSION_V2, ActiveMode, ArtifactMeta, SidecarDescribe, SidecarPhase,
    SidecarRuntimeState,
};
use crate::events::{Event, EventSink, diag, ms_to_iso8601, now_ms, now_ts};
use crate::host::{Artifact, ArtifactStore, HostState};

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

// ── SidecarController ────────────────────────────────────────────────

/// Owns a running WASM sidecar module and provides an observable runtime state.
pub struct SidecarController {
    /// Canonical runtime state; read by `snapshot()`.
    runtime_state: Arc<Mutex<SidecarRuntimeState>>,
    /// Named artifact store; published_output consumed by `--once` mode.
    artifact_store: Arc<Mutex<ArtifactStore>>,
    /// Background thread running the sidecar.
    thread: Option<thread::JoinHandle<()>>,
    /// Signal to gracefully stop the sidecar thread.
    stop_signal: Arc<AtomicBool>,
    /// When set, the next `announce_sleep` call returns 1 (skip sleep).
    force_refresh: Arc<AtomicBool>,
    /// Current JSON params exposed through host `params_len`/`params_read` imports.
    params_bytes: Arc<Mutex<Option<Vec<u8>>>>,
}

impl SidecarController {
    /// Load a sidecar WASM module and start running it in a background thread.
    pub fn new(
        wasm_path: &str,
        env_vars: Vec<(String, String)>,
        params_bytes: Option<Vec<u8>>,
        log_channel: bool,
        event_sink: EventSink,
    ) -> Result<Self> {
        let wasm_bytes =
            std::fs::read(wasm_path).with_context(|| format!("read WASM file: {wasm_path}"))?;

        let runtime_state = Arc::new(Mutex::new(SidecarRuntimeState::default()));
        let stop_signal = Arc::new(AtomicBool::new(false));

        let engine = Engine::default();
        let module = Module::from_binary(&engine, &wasm_bytes)
            .with_context(|| format!("compile WASM module: {wasm_path}"))?;

        // Detect ABI version by checking for the v2 export.
        let abi_version = detect_abi_version(&module);

        // Update mode in runtime state.
        {
            let mut state = runtime_state.lock().unwrap();
            state.mode = match abi_version {
                ABI_VERSION_V2 => ActiveMode::ManagedPolling,
                _ => ActiveMode::V1Legacy,
            };
        }

        // Build shared stores.
        let artifact_store = Arc::new(Mutex::new(ArtifactStore::default()));
        let force_refresh = Arc::new(AtomicBool::new(false));
        let params_state = Arc::new(Mutex::new(params_bytes.clone()));
        let artifact_store_clone = artifact_store.clone();
        let force_refresh_clone = force_refresh.clone();
        let params_state_clone = params_state.clone();
        let runtime_state_clone = runtime_state.clone();
        let stop_clone = stop_signal.clone();
        let wasm_path_str = wasm_path.to_string();

        let handle = thread::spawn(move || {
            let result = run_wasm_instance(
                &engine,
                &module,
                &wasm_path_str,
                artifact_store_clone,
                runtime_state_clone,
                env_vars,
                params_bytes,
                params_state_clone,
                log_channel,
                event_sink,
                abi_version,
                wasm_bytes.len(),
                stop_clone,
                force_refresh_clone,
            );
            if let Err(e) = result {
                eprintln!("[brrmmmm] WASM execution error: {e:?}");
            }
        });

        Ok(Self {
            runtime_state,
            artifact_store,
            thread: Some(handle),
            stop_signal,
            force_refresh,
            params_bytes: params_state,
        })
    }

    /// Return a snapshot of the current runtime state.
    pub fn snapshot(&self) -> SidecarRuntimeState {
        self.runtime_state.lock().unwrap().clone()
    }

    /// Poll for the latest published_output artifact, consuming it.
    pub fn poll_output(&self) -> Option<Vec<u8>> {
        self.artifact_store
            .lock()
            .unwrap()
            .take_published()
            .map(|a| a.data)
    }

    /// Return a clone of the force-refresh flag for use by the stdin command listener.
    pub fn force_refresh_flag(&self) -> Arc<AtomicBool> {
        self.force_refresh.clone()
    }

    /// Return a clone of the current params handle for command listeners.
    pub fn params_handle(&self) -> Arc<Mutex<Option<Vec<u8>>>> {
        self.params_bytes.clone()
    }

    /// Signal the sidecar to stop and detach the background thread.
    pub fn stop(mut self) {
        self.stop_signal.store(true, Ordering::Relaxed);
        self.thread.take();
    }
}

impl Drop for SidecarController {
    fn drop(&mut self) {
        self.stop_signal.store(true, Ordering::Relaxed);
        self.thread.take();
    }
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

fn read_static_describe(
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

// ── WASM instance runner ─────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn run_wasm_instance(
    engine: &Engine,
    module: &Module,
    wasm_path: &str,
    artifact_store: Arc<Mutex<ArtifactStore>>,
    runtime_state: Arc<Mutex<SidecarRuntimeState>>,
    env_vars: Vec<(String, String)>,
    params_bytes: Option<Vec<u8>>,
    params_state: Arc<Mutex<Option<Vec<u8>>>>,
    log_channel: bool,
    event_sink: EventSink,
    abi_version: u32,
    wasm_size_bytes: usize,
    stop_signal: Arc<AtomicBool>,
    force_refresh: Arc<AtomicBool>,
) -> Result<()> {
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
        wasm_path: wasm_path.to_string(),
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
                runtime_state.lock().unwrap().describe = Some(describe);
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
) -> Result<Option<SidecarDescribe>> {
    read_static_describe(instance, store)
}

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

// ── Linker registration ──────────────────────────────────────────────

fn register_vzglyd_host_on_linker(
    linker: &mut Linker<wasmtime_wasi::preview1::WasiP1Ctx>,
    host_state: HostState,
    event_sink: EventSink,
    runtime_state: Arc<Mutex<SidecarRuntimeState>>,
    stop_signal: Arc<AtomicBool>,
    force_refresh: Arc<AtomicBool>,
) -> Result<()> {
    let shared = Arc::new(Mutex::new(host_state));

    // Request ID counter for correlating request_start / request_done events.
    let request_counter = Arc::new(AtomicU64::new(0));

    // ── channel_push ─────────────────────────────────────────────────
    // v1 alias for artifact_publish("published_output", data).
    let s_push = shared.clone();
    let sink_push = event_sink.clone();
    let runtime_push = runtime_state.clone();
    linker.func_wrap(
        "vzglyd_host",
        "channel_push",
        move |mut caller: wasmtime::Caller<'_, wasmtime_wasi::preview1::WasiP1Ctx>,
              ptr: i32,
              len: i32|
              -> i32 {
            let data = match read_memory_from_caller(&mut caller, ptr, len) {
                Ok(d) => d,
                Err(e) => {
                    diag(
                        &sink_push,
                        &format!("[brrmmmm] channel_push memory error: {e}"),
                    );
                    return -1;
                }
            };
            let size = data.len();
            let received_at = now_ms();

            let preview = String::from_utf8_lossy(&data).into_owned();

            let artifact = Artifact {
                kind: "published_output".to_string(),
                data,
                received_at_ms: received_at,
            };
            let meta = ArtifactMeta {
                kind: "published_output".to_string(),
                size_bytes: size,
                received_at_ms: received_at,
            };

            {
                let guard = s_push.lock().unwrap();
                if guard.log_channel {
                    diag(&sink_push, &format!("[brrmmmm] channel_push: {size} bytes"));
                    diag(
                        &sink_push,
                        &format!(
                            "[brrmmmm]   payload: {}",
                            &preview.chars().take(200).collect::<String>()
                        ),
                    );
                }
                guard.artifact_store.lock().unwrap().store(artifact);
            }

            update_artifact_state(&runtime_push, &meta);
            update_phase_state(&runtime_push, &sink_push, SidecarPhase::Publishing);
            sink_push.emit(Event::ArtifactReceived {
                ts: now_ts(),
                kind: "published_output".to_string(),
                size_bytes: size,
                preview,
                artifact: meta,
            });
            0
        },
    )?;

    linker.func_wrap(
        "vzglyd_host",
        "channel_poll",
        |_caller: wasmtime::Caller<'_, wasmtime_wasi::preview1::WasiP1Ctx>,
         _ptr: i32,
         _len: i32|
         -> i32 { -1 },
    )?;

    linker.func_wrap(
        "vzglyd_host",
        "channel_active",
        |_caller: wasmtime::Caller<'_, wasmtime_wasi::preview1::WasiP1Ctx>| -> i32 { 1 },
    )?;

    // ── artifact_publish ─────────────────────────────────────────────
    // v2 named artifact publication.
    let s_artifact = shared.clone();
    let sink_artifact = event_sink.clone();
    let runtime_artifact = runtime_state.clone();
    linker.func_wrap(
        "vzglyd_host",
        "artifact_publish",
        move |mut caller: wasmtime::Caller<'_, wasmtime_wasi::preview1::WasiP1Ctx>,
              kind_ptr: i32,
              kind_len: i32,
              data_ptr: i32,
              data_len: i32|
              -> i32 {
            let kind_bytes = match read_memory_from_caller(&mut caller, kind_ptr, kind_len) {
                Ok(b) => b,
                Err(_) => return -1,
            };
            let kind = String::from_utf8_lossy(&kind_bytes).into_owned();

            let data = match read_memory_from_caller(&mut caller, data_ptr, data_len) {
                Ok(d) => d,
                Err(e) => {
                    diag(
                        &sink_artifact,
                        &format!("[brrmmmm] artifact_publish memory error: {e}"),
                    );
                    return -1;
                }
            };
            let size = data.len();
            let received_at = now_ms();
            let preview = String::from_utf8_lossy(&data).into_owned();

            let artifact = Artifact {
                kind: kind.clone(),
                data,
                received_at_ms: received_at,
            };
            let meta = ArtifactMeta {
                kind: kind.clone(),
                size_bytes: size,
                received_at_ms: received_at,
            };

            s_artifact
                .lock()
                .unwrap()
                .artifact_store
                .lock()
                .unwrap()
                .store(artifact);

            update_artifact_state(&runtime_artifact, &meta);
            update_phase_state(&runtime_artifact, &sink_artifact, SidecarPhase::Publishing);
            sink_artifact.emit(Event::ArtifactReceived {
                ts: now_ts(),
                kind,
                size_bytes: size,
                preview,
                artifact: meta,
            });
            0
        },
    )?;

    // ── params_len / params_read ─────────────────────────────────────
    // Current host-owned JSON params. Unlike the legacy configure buffer, these can change while
    // the sidecar is alive and will be picked up by sidecars that read params each poll cycle.
    let s_params_len = shared.clone();
    linker.func_wrap("vzglyd_host", "params_len", move || -> i32 {
        s_params_len
            .lock()
            .unwrap()
            .params_bytes
            .lock()
            .unwrap()
            .as_ref()
            .map_or(0, |params| params.len() as i32)
    })?;

    let s_params_read = shared.clone();
    let sink_params_read = event_sink.clone();
    linker.func_wrap(
        "vzglyd_host",
        "params_read",
        move |mut caller: wasmtime::Caller<'_, wasmtime_wasi::preview1::WasiP1Ctx>,
              ptr: i32,
              len: i32|
              -> i32 {
            if ptr < 0 || len < 0 {
                return -1;
            }
            let params = s_params_read
                .lock()
                .unwrap()
                .params_bytes
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_default();
            if params.len() > len as usize {
                return -2;
            }
            match write_memory_from_caller(&mut caller, ptr, &params) {
                Ok(()) => params.len() as i32,
                Err(error) => {
                    diag(
                        &sink_params_read,
                        &format!("[brrmmmm] params_read memory error: {error}"),
                    );
                    -1
                }
            }
        },
    )?;

    // ── log_info ─────────────────────────────────────────────────────
    let sink_log = event_sink.clone();
    linker.func_wrap(
        "vzglyd_host",
        "log_info",
        move |mut caller: wasmtime::Caller<'_, wasmtime_wasi::preview1::WasiP1Ctx>,
              ptr: i32,
              len: i32|
              -> i32 {
            if let Ok(data) = read_memory_from_caller(&mut caller, ptr, len) {
                if let Ok(msg) = std::str::from_utf8(&data) {
                    if sink_log.is_enabled() {
                        sink_log.emit(Event::Log {
                            ts: now_ts(),
                            message: msg.to_string(),
                        });
                    } else {
                        eprintln!("[sidecar] {msg}");
                    }
                }
            }
            0
        },
    )?;

    // ── sleep_ms ─────────────────────────────────────────────────────
    // Host-controlled sleep. This replaces guest-side sleeping so force-refresh can wake the
    // sidecar immediately instead of waiting for the original sleep duration.
    let sink_host_sleep = event_sink.clone();
    let force_refresh_host_sleep = force_refresh.clone();
    let stop_host_sleep = stop_signal.clone();
    let runtime_host_sleep = runtime_state.clone();
    linker.func_wrap(
        "vzglyd_host",
        "sleep_ms",
        move |_caller: wasmtime::Caller<'_, wasmtime_wasi::preview1::WasiP1Ctx>,
              duration_ms: i64|
              -> i32 {
            if duration_ms <= 0 {
                return 0;
            }
            let duration_ms = duration_ms as u64;
            let wake_ms = now_ms().saturating_add(duration_ms);
            update_sleep_state(&runtime_host_sleep, &sink_host_sleep, duration_ms, wake_ms);
            sink_host_sleep.emit(Event::SleepStart {
                ts: now_ts(),
                duration_ms: duration_ms as i64,
                wake_at: ms_to_iso8601(wake_ms),
            });

            let started = Instant::now();
            let total = Duration::from_millis(duration_ms);
            loop {
                if stop_host_sleep.load(Ordering::Relaxed) {
                    return 1;
                }
                if force_refresh_host_sleep.swap(false, Ordering::Relaxed) {
                    return 1;
                }
                let elapsed = started.elapsed();
                if elapsed >= total {
                    return 0;
                }
                let remaining = total.saturating_sub(elapsed);
                thread::sleep(remaining.min(Duration::from_millis(100)));
            }
        },
    )?;

    // ── announce_sleep ───────────────────────────────────────────────
    // Called by sidecar before sleeping. Enables TUI countdown.
    // Returns 1 if force_refresh was requested (SDK should skip sleep).
    let sink_sleep = event_sink.clone();
    let force_refresh_sleep = force_refresh.clone();
    let runtime_sleep = runtime_state.clone();
    linker.func_wrap(
        "vzglyd_host",
        "announce_sleep",
        move |_caller: wasmtime::Caller<'_, wasmtime_wasi::preview1::WasiP1Ctx>,
              duration_ms: i64|
              -> i32 {
            // If force-refresh was requested, clear the flag and tell the
            // SDK to skip its sleep entirely (return value 1 = skip sleep).
            if force_refresh_sleep.swap(false, Ordering::Relaxed) {
                return 1;
            }
            let wake_ms = now_ms().saturating_add(duration_ms.unsigned_abs());
            let wake_at = ms_to_iso8601(wake_ms);
            update_sleep_state(
                &runtime_sleep,
                &sink_sleep,
                duration_ms.unsigned_abs(),
                wake_ms,
            );
            sink_sleep.emit(Event::SleepStart {
                ts: now_ts(),
                duration_ms,
                wake_at,
            });
            0
        },
    )?;

    // ── register_manifest ────────────────────────────────────────────
    // Legacy v1 host import. Parsed and emitted as Describe event.
    let sink_manifest = event_sink.clone();
    let runtime_manifest = runtime_state.clone();
    linker.func_wrap(
        "vzglyd_host",
        "register_manifest",
        move |mut caller: wasmtime::Caller<'_, wasmtime_wasi::preview1::WasiP1Ctx>,
              ptr: i32,
              len: i32|
              -> i32 {
            if let Ok(data) = read_memory_from_caller(&mut caller, ptr, len) {
                if let Ok(describe) = serde_json::from_slice::<SidecarDescribe>(&data) {
                    runtime_manifest.lock().unwrap().describe = Some(describe.clone());
                    sink_manifest.emit(Event::Describe {
                        ts: now_ts(),
                        describe,
                    });
                }
            }
            0
        },
    )?;

    // ── network_request ──────────────────────────────────────────────
    let s_net = shared.clone();
    let sink_net = event_sink.clone();
    let counter_net = request_counter.clone();
    let runtime_net = runtime_state.clone();
    linker.func_wrap(
        "vzglyd_host",
        "network_request",
        move |mut caller: wasmtime::Caller<'_, wasmtime_wasi::preview1::WasiP1Ctx>,
              ptr: i32,
              len: i32|
              -> i32 {
            let req_bytes = match read_memory_from_caller(&mut caller, ptr, len) {
                Ok(b) => b,
                Err(e) => {
                    diag(
                        &sink_net,
                        &format!("[brrmmmm] network_request memory error: {e}"),
                    );
                    return -1;
                }
            };

            let decoded: serde_json::Value = match serde_json::from_slice(&req_bytes) {
                Ok(v) => v,
                Err(e) => {
                    diag(
                        &sink_net,
                        &format!("[brrmmmm] network_request decode error: {e}"),
                    );
                    return -1;
                }
            };

            let wire_version = decoded
                .get("wire_version")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u8;
            if wire_version != crate::host::host_request::WIRE_VERSION {
                diag(
                    &sink_net,
                    &format!("[brrmmmm] network_request wire_version mismatch: {wire_version}"),
                );
                return -1;
            }

            let request: crate::host::host_request::HostRequest =
                match serde_json::from_value(decoded) {
                    Ok(r) => r,
                    Err(e) => {
                        diag(
                            &sink_net,
                            &format!("[brrmmmm] network_request parse error: {e}"),
                        );
                        return -1;
                    }
                };

            // Emit RequestStart before any I/O.
            let req_id = counter_net.fetch_add(1, Ordering::Relaxed);
            let request_id = format!("r{req_id}");
            let (req_kind, req_host, req_path) = describe_request(&request);
            update_phase_state(&runtime_net, &sink_net, SidecarPhase::Fetching);
            sink_net.emit(Event::RequestStart {
                ts: now_ts(),
                request_id: request_id.clone(),
                kind: req_kind,
                host: req_host,
                path: req_path,
            });

            let start = Instant::now();
            let response = match execute_native_request(&request) {
                Ok(resp) => resp,
                Err(e) => {
                    update_failure_state(&runtime_net, &e);
                    sink_net.emit(Event::RequestError {
                        ts: now_ts(),
                        request_id: request_id.clone(),
                        error_kind: "io".to_string(),
                        message: e.clone(),
                    });
                    let response = crate::host::host_request::HostResponse::Error {
                        error_kind: crate::host::host_request::ErrorKind::Io,
                        message: e,
                    };
                    let resp_bytes = encode_response_for_sidecar(&response);
                    let guard = s_net.lock().unwrap();
                    *guard.pending_response.lock().unwrap() = Some(resp_bytes);
                    return 0;
                }
            };

            let elapsed_ms = start.elapsed().as_millis() as u64;

            // Emit RequestDone (even for error responses the request completed).
            let (status_code, response_size) = response_info(&response);
            sink_net.emit(Event::RequestDone {
                ts: now_ts(),
                request_id,
                status_code,
                elapsed_ms,
                response_size_bytes: response_size,
            });

            // Auto-publish the raw HTTP response body so the TUI RAW pane
            // is populated without requiring sidecars to call publish_raw().
            if let crate::host::host_request::HostResponse::Http {
                status_code, body, ..
            } = &response
            {
                if *status_code < 400 {
                    let received_at_ms = now_ms();
                    let preview = String::from_utf8_lossy(body).into_owned();
                    let raw_artifact = Artifact {
                        kind: "raw_source_payload".to_string(),
                        data: body.clone(),
                        received_at_ms,
                    };
                    s_net
                        .lock()
                        .unwrap()
                        .artifact_store
                        .lock()
                        .unwrap()
                        .store(raw_artifact);
                    let meta = ArtifactMeta {
                        kind: "raw_source_payload".to_string(),
                        size_bytes: body.len(),
                        received_at_ms,
                    };
                    update_artifact_state(&runtime_net, &meta);
                    sink_net.emit(Event::ArtifactReceived {
                        ts: now_ts(),
                        kind: "raw_source_payload".to_string(),
                        size_bytes: body.len(),
                        preview,
                        artifact: meta,
                    });
                }
            }

            let resp_bytes = encode_response_for_sidecar(&response);
            let guard = s_net.lock().unwrap();
            *guard.pending_response.lock().unwrap() = Some(resp_bytes);
            0
        },
    )?;

    let s_resp = shared.clone();
    linker.func_wrap(
        "vzglyd_host",
        "network_response_len",
        move |_caller: wasmtime::Caller<'_, wasmtime_wasi::preview1::WasiP1Ctx>| -> i32 {
            let guard = s_resp.lock().unwrap();
            guard
                .pending_response
                .lock()
                .unwrap()
                .as_ref()
                .map(|b| b.len() as i32)
                .unwrap_or(-1)
        },
    )?;

    let s_read = shared.clone();
    linker.func_wrap(
        "vzglyd_host",
        "network_response_read",
        move |mut caller: wasmtime::Caller<'_, wasmtime_wasi::preview1::WasiP1Ctx>,
              ptr: i32,
              len: i32|
              -> i32 {
            let guard = s_read.lock().unwrap();
            let mut resp_guard = guard.pending_response.lock().unwrap();
            let Some(data) = resp_guard.take() else {
                return -1;
            };
            let write_len = std::cmp::min(data.len(), len as usize);
            if let Err(e) = write_memory_from_caller(&mut caller, ptr, &data[..write_len]) {
                eprintln!("[brrmmmm] network_response_read error: {e}");
                return -1;
            }
            write_len as i32
        },
    )?;

    // ── Tracing (no-op stubs) ─────────────────────────────────────────
    linker.func_wrap(
        "vzglyd_host",
        "trace_span_start",
        |_caller: wasmtime::Caller<'_, wasmtime_wasi::preview1::WasiP1Ctx>,
         _ptr: i32,
         _len: i32|
         -> i32 { 1 },
    )?;

    linker.func_wrap(
        "vzglyd_host",
        "trace_span_end",
        |_caller: wasmtime::Caller<'_, wasmtime_wasi::preview1::WasiP1Ctx>,
         _span_id: i32,
         _ptr: i32,
         _len: i32|
         -> i32 { 0 },
    )?;

    linker.func_wrap(
        "vzglyd_host",
        "trace_event",
        |_caller: wasmtime::Caller<'_, wasmtime_wasi::preview1::WasiP1Ctx>,
         _ptr: i32,
         _len: i32|
         -> i32 { 0 },
    )?;

    Ok(())
}

// ── Runtime state helpers ────────────────────────────────────────────

fn update_phase_state(
    runtime_state: &Arc<Mutex<SidecarRuntimeState>>,
    event_sink: &EventSink,
    phase: SidecarPhase,
) {
    runtime_state.lock().unwrap().phase = phase.clone();
    event_sink.emit(Event::Phase {
        ts: now_ts(),
        phase,
    });
}

fn update_sleep_state(
    runtime_state: &Arc<Mutex<SidecarRuntimeState>>,
    event_sink: &EventSink,
    duration_ms: u64,
    wake_ms: u64,
) {
    {
        let mut state = runtime_state.lock().unwrap();
        state.phase = SidecarPhase::CoolingDown;
        state.next_scheduled_poll_at_ms = Some(wake_ms);
        state.cooldown_until_ms = Some(wake_ms);
        state.backoff_ms = Some(duration_ms);
    }
    event_sink.emit(Event::Phase {
        ts: now_ts(),
        phase: SidecarPhase::CoolingDown,
    });
}

fn update_artifact_state(runtime_state: &Arc<Mutex<SidecarRuntimeState>>, meta: &ArtifactMeta) {
    let mut state = runtime_state.lock().unwrap();
    match meta.kind.as_str() {
        "raw_source_payload" => state.last_raw_artifact = Some(meta.clone()),
        "published_output" => {
            state.last_output_artifact = Some(meta.clone());
            state.last_success_at_ms = Some(meta.received_at_ms);
            state.consecutive_failures = 0;
            state.last_error = None;
        }
        _ => {}
    }
}

fn update_failure_state(runtime_state: &Arc<Mutex<SidecarRuntimeState>>, error: &str) {
    let mut state = runtime_state.lock().unwrap();
    state.phase = SidecarPhase::Failed;
    state.last_failure_at_ms = Some(now_ms());
    state.consecutive_failures = state.consecutive_failures.saturating_add(1);
    state.last_error = Some(error.to_string());
}

// ── Request helpers ──────────────────────────────────────────────────

fn describe_request(
    req: &crate::host::host_request::HostRequest,
) -> (String, String, Option<String>) {
    use crate::host::host_request::HostRequest;
    match req {
        HostRequest::HttpsGet { host, path, .. } => {
            ("https_get".to_string(), host.clone(), Some(path.clone()))
        }
        HostRequest::TcpConnect { host, port, .. } => {
            ("tcp_connect".to_string(), format!("{host}:{port}"), None)
        }
    }
}

fn response_info(resp: &crate::host::host_request::HostResponse) -> (Option<u16>, usize) {
    use crate::host::host_request::HostResponse;
    match resp {
        HostResponse::Http {
            status_code, body, ..
        } => (Some(*status_code), body.len()),
        HostResponse::TcpConnect { .. } => (None, 0),
        HostResponse::Error { .. } => (None, 0),
    }
}

// ── Memory helpers ───────────────────────────────────────────────────

fn read_memory_from_caller(
    caller: &mut wasmtime::Caller<'_, wasmtime_wasi::preview1::WasiP1Ctx>,
    ptr: i32,
    len: i32,
) -> anyhow::Result<Vec<u8>> {
    let mem = caller
        .get_export("memory")
        .and_then(|m| m.into_memory())
        .ok_or_else(|| anyhow::anyhow!("no memory export"))?;
    let data = mem
        .data(caller)
        .get(ptr as usize..)
        .and_then(|s| s.get(..len as usize))
        .ok_or_else(|| anyhow::anyhow!("memory read OOB: ptr={ptr}, len={len}"))?;
    Ok(data.to_vec())
}

fn write_memory_from_caller(
    caller: &mut wasmtime::Caller<'_, wasmtime_wasi::preview1::WasiP1Ctx>,
    ptr: i32,
    data: &[u8],
) -> anyhow::Result<()> {
    let mem = caller
        .get_export("memory")
        .and_then(|m| m.into_memory())
        .ok_or_else(|| anyhow::anyhow!("no memory export"))?;
    let mem_data = mem.data_mut(caller);
    let dst = mem_data
        .get_mut(ptr as usize..)
        .and_then(|s| s.get_mut(..data.len()))
        .ok_or_else(|| anyhow::anyhow!("memory write OOB: ptr={ptr}, len={}", data.len()))?;
    dst.copy_from_slice(data);
    Ok(())
}

// ── Response encoding ────────────────────────────────────────────────

fn encode_response_for_sidecar(response: &crate::host::host_request::HostResponse) -> Vec<u8> {
    use crate::host::host_request::HostResponse;
    match response {
        HostResponse::Http { status_code, headers, body } => serde_json::json!({
            "wire_version": 1u8,
            "kind": "http",
            "status_code": status_code,
            "headers": headers.iter().map(|h| serde_json::json!({"name": h.name, "value": h.value})).collect::<Vec<_>>(),
            "body": body.iter().map(|&b| b as u64).collect::<Vec<_>>(),
        }),
        HostResponse::TcpConnect { elapsed_ms } => serde_json::json!({
            "wire_version": 1u8,
            "kind": "tcp_connect",
            "elapsed_ms": elapsed_ms,
        }),
        HostResponse::Error { error_kind, message } => serde_json::json!({
            "wire_version": 1u8,
            "kind": "error",
            "error_kind": format!("{error_kind:?}").to_lowercase(),
            "message": message,
        }),
    }
    .to_string()
    .into_bytes()
}

// ── Native request execution ─────────────────────────────────────────

fn execute_native_request(
    req: &crate::host::host_request::HostRequest,
) -> Result<crate::host::host_request::HostResponse, String> {
    use crate::host::host_request::{Header, HostRequest, HostResponse};

    match req {
        HostRequest::HttpsGet {
            host,
            path,
            headers,
        } => {
            let url = format!("https://{host}{path}");

            let mut builder = reqwest::blocking::Client::builder()
                .use_rustls_tls()
                .timeout(std::time::Duration::from_secs(30));

            if !headers.is_empty() {
                let mut hm = reqwest::header::HeaderMap::new();
                for h in headers {
                    if let (Ok(n), Ok(v)) = (
                        reqwest::header::HeaderName::from_bytes(h.name.as_bytes()),
                        reqwest::header::HeaderValue::from_bytes(h.value.as_bytes()),
                    ) {
                        hm.insert(n, v);
                    }
                }
                builder = builder.default_headers(hm);
            }

            let client = builder.build().map_err(|e| format!("build client: {e}"))?;
            let resp = client
                .get(&url)
                .send()
                .map_err(|e| format!("request: {e}"))?;
            let status_code = resp.status().as_u16();

            let resp_headers: Vec<Header> = resp
                .headers()
                .iter()
                .filter_map(|(n, v)| {
                    Some(Header {
                        name: n.as_str().to_string(),
                        value: v.to_str().ok()?.to_string(),
                    })
                })
                .collect();

            let body = resp
                .bytes()
                .map_err(|e| format!("read body: {e}"))?
                .to_vec();

            Ok(HostResponse::Http {
                status_code,
                headers: resp_headers,
                body,
            })
        }
        HostRequest::TcpConnect {
            host,
            port,
            timeout_ms,
        } => {
            let addr = format!("{host}:{port}");
            let start = std::time::Instant::now();
            let timeout = std::time::Duration::from_millis(*timeout_ms as u64);
            let _stream = std::net::TcpStream::connect_timeout(
                &addr.parse().map_err(|e| format!("parse addr: {e}"))?,
                timeout,
            )
            .map_err(|e| format!("connect: {e}"))?;
            Ok(HostResponse::TcpConnect {
                elapsed_ms: start.elapsed().as_millis() as u64,
            })
        }
    }
}
