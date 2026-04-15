use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::thread;
use std::time::Instant;

use anyhow::{Context, Result};
use wasmtime::{Engine, Linker, Module, Store};

use crate::abi::{
    ABI_VERSION_V1, ABI_VERSION_V2, ActiveMode, ArtifactMeta, SidecarDescribe,
    SidecarRuntimeState,
};
use crate::events::{diag, ms_to_iso8601, now_ms, now_ts, Event, EventSink};
use crate::host::{Artifact, ArtifactStore, HostState};

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
}

impl SidecarController {
    /// Load a sidecar WASM module and start running it in a background thread.
    pub fn new(
        wasm_path: &str,
        env_vars: Vec<(String, String)>,
        log_channel: bool,
        event_sink: EventSink,
    ) -> Result<Self> {
        let wasm_bytes = std::fs::read(wasm_path)
            .with_context(|| format!("read WASM file: {wasm_path}"))?;

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
        let artifact_store_clone = artifact_store.clone();
        let force_refresh_clone = force_refresh.clone();
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
                log_channel,
                event_sink,
                abi_version,
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

    /// Request that the sidecar skip its next sleep and poll immediately.
    pub fn request_force_refresh(&self) {
        self.force_refresh.store(true, Ordering::Relaxed);
    }

    /// Signal the sidecar to stop and wait for the thread to join.
    pub fn stop(mut self) {
        self.stop_signal.store(true, Ordering::Relaxed);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for SidecarController {
    fn drop(&mut self) {
        self.stop_signal.store(true, Ordering::Relaxed);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

// ── ABI version detection ────────────────────────────────────────────

fn detect_abi_version(module: &Module) -> u32 {
    let has_v2 = module
        .exports()
        .any(|e| e.name() == "vzglyd_sidecar_abi_version");
    if has_v2 { ABI_VERSION_V2 } else { ABI_VERSION_V1 }
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
    log_channel: bool,
    event_sink: EventSink,
    abi_version: u32,
    _stop_signal: Arc<AtomicBool>,
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
    let mut host_state = HostState::new(log_channel);
    // Share the artifact_store from the controller.
    host_state.artifact_store = artifact_store;

    register_vzglyd_host_on_linker(&mut linker, host_state, event_sink.clone(), force_refresh)?;

    let instance = linker
        .instantiate(&mut store, module)
        .context("instantiate WASM module")?;

    // Emit Started event (before calling the entry point).
    let wasm_size = module
        .exports()
        .count(); // rough proxy; real size is the original bytes length
    let _ = wasm_size; // suppress unused
    event_sink.emit(Event::Started {
        ts: now_ts(),
        wasm_path: wasm_path.to_string(),
        wasm_size_bytes: 0, // populated externally if needed
        abi_version,
    });

    // For v2: call describe() if available and update runtime_state.
    if abi_version == ABI_VERSION_V2 {
        if let Some(describe) = call_describe(&instance, &mut store) {
            event_sink.emit(Event::Describe {
                ts: now_ts(),
                describe: describe.clone(),
            });
            runtime_state.lock().unwrap().describe = Some(describe);
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

    call_result.map(|_| ()).map_err(|e| anyhow::anyhow!("{e:?}"))
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
) -> Option<SidecarDescribe> {
    let describe_fn = instance.get_func(&mut *store, "vzglyd_sidecar_describe")?;

    // vzglyd_sidecar_describe(out_ptr: *mut i32, out_len: *mut i32) -> i32
    // The guest writes a JSON blob into its own memory and sets out_ptr/out_len.
    // We pass two i32 scratch locations; the guest fills them in.
    // For Sprint 1 we use a simplified calling convention: the guest may not yet
    // implement this. Return None gracefully if the call fails or returns non-zero.
    let mut results = vec![wasmtime::Val::I32(0)];
    let params = [wasmtime::Val::I32(0), wasmtime::Val::I32(0)];
    describe_fn.call(store, &params, &mut results).ok()?;
    None // Full v2 describe parsing is Sprint 2; for now just detect presence.
}

// ── Linker registration ──────────────────────────────────────────────

fn register_vzglyd_host_on_linker(
    linker: &mut Linker<wasmtime_wasi::preview1::WasiP1Ctx>,
    host_state: HostState,
    event_sink: EventSink,
    force_refresh: Arc<AtomicBool>,
) -> Result<()> {
    let shared = Arc::new(Mutex::new(host_state));

    // Request ID counter for correlating request_start / request_done events.
    let request_counter = Arc::new(AtomicU64::new(0));

    // ── channel_push ─────────────────────────────────────────────────
    // v1 alias for artifact_publish("published_output", data).
    let s_push = shared.clone();
    let sink_push = event_sink.clone();
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
                    diag(&sink_push, &format!("[brrmmmm] channel_push memory error: {e}"));
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
                    diag(&sink_push, &format!("[brrmmmm]   payload: {}", &preview.chars().take(200).collect::<String>()));
                }
                guard.artifact_store.lock().unwrap().store(artifact);
            }

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
    // v2 named artifact publication. In Sprint 1, registers as a no-op with event emission.
    let s_artifact = shared.clone();
    let sink_artifact = event_sink.clone();
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
                    diag(&sink_artifact, &format!("[brrmmmm] artifact_publish memory error: {e}"));
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

            s_artifact.lock().unwrap().artifact_store.lock().unwrap().store(artifact);

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

    // ── announce_sleep ───────────────────────────────────────────────
    // Called by sidecar before sleeping. Enables TUI countdown.
    // Returns 1 if force_refresh was requested (SDK should skip sleep).
    let sink_sleep = event_sink.clone();
    let force_refresh_sleep = force_refresh.clone();
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
    linker.func_wrap(
        "vzglyd_host",
        "register_manifest",
        move |mut caller: wasmtime::Caller<'_, wasmtime_wasi::preview1::WasiP1Ctx>,
              ptr: i32,
              len: i32|
              -> i32 {
            if let Ok(data) = read_memory_from_caller(&mut caller, ptr, len) {
                if let Ok(describe) = serde_json::from_slice::<SidecarDescribe>(&data) {
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
                    diag(&sink_net, &format!("[brrmmmm] network_request memory error: {e}"));
                    return -1;
                }
            };

            let decoded: serde_json::Value = match serde_json::from_slice(&req_bytes) {
                Ok(v) => v,
                Err(e) => {
                    diag(&sink_net, &format!("[brrmmmm] network_request decode error: {e}"));
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
                        diag(&sink_net, &format!("[brrmmmm] network_request parse error: {e}"));
                        return -1;
                    }
                };

            // Emit RequestStart before any I/O.
            let req_id = counter_net.fetch_add(1, Ordering::Relaxed);
            let request_id = format!("r{req_id}");
            let (req_kind, req_host, req_path) = describe_request(&request);
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
                    sink_net.emit(Event::RequestError {
                        ts: now_ts(),
                        request_id: request_id.clone(),
                        error_kind: "io".to_string(),
                        message: e.clone(),
                    });
                    crate::host::host_request::HostResponse::Error {
                        error_kind: crate::host::host_request::ErrorKind::Io,
                        message: e,
                    }
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
            if let crate::host::host_request::HostResponse::Http { status_code, body, .. } = &response {
                if *status_code < 400 {
                    let received_at_ms = now_ms();
                    let preview = String::from_utf8_lossy(body).into_owned();
                    let raw_artifact = Artifact {
                        kind: "raw_source_payload".to_string(),
                        data: body.clone(),
                        received_at_ms,
                    };
                    s_net.lock().unwrap().artifact_store.lock().unwrap().store(raw_artifact);
                    sink_net.emit(Event::ArtifactReceived {
                        ts: now_ts(),
                        kind: "raw_source_payload".to_string(),
                        size_bytes: body.len(),
                        preview,
                        artifact: ArtifactMeta {
                            kind: "raw_source_payload".to_string(),
                            size_bytes: body.len(),
                            received_at_ms,
                        },
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

fn response_info(
    resp: &crate::host::host_request::HostResponse,
) -> (Option<u16>, usize) {
    use crate::host::host_request::HostResponse;
    match resp {
        HostResponse::Http { status_code, body, .. } => (Some(*status_code), body.len()),
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
        .ok_or_else(|| {
            anyhow::anyhow!("memory write OOB: ptr={ptr}, len={}", data.len())
        })?;
    dst.copy_from_slice(data);
    Ok(())
}

// ── Response encoding ────────────────────────────────────────────────

fn encode_response_for_sidecar(
    response: &crate::host::host_request::HostResponse,
) -> Vec<u8> {
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
        HostRequest::HttpsGet { host, path, headers } => {
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
        HostRequest::TcpConnect { host, port, timeout_ms } => {
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
