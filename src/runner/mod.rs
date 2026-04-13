use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{Context, Result};
use wasmtime::{Engine, Linker, Module, Store};

use crate::host::HostState;

/// Runner that loads and executes a sidecar WASM module.
pub struct SidecarRunner {
    /// The channel data pushed by the sidecar. Cleared after each read.
    channel_data: Arc<Mutex<Option<Vec<u8>>>>,
    /// Handle to the background thread running the sidecar.
    thread: Option<thread::JoinHandle<()>>,
    /// Signal to stop the sidecar thread.
    stop_signal: Arc<Mutex<bool>>,
}

impl SidecarRunner {
    /// Load a sidecar WASM module and start running it in a background thread.
    pub fn new(
        wasm_path: &str,
        env_vars: Vec<(String, String)>,
        log_channel: bool,
    ) -> Result<Self> {
        let wasm_bytes = std::fs::read(wasm_path)
            .with_context(|| format!("read WASM file: {wasm_path}"))?;

        let channel_data = Arc::new(Mutex::new(None));
        let stop_signal = Arc::new(Mutex::new(false));

        let engine = Engine::default();
        let module = Module::from_binary(&engine, &wasm_bytes)
            .with_context(|| format!("compile WASM module: {wasm_path}"))?;

        let channel_clone = channel_data.clone();
        let stop_clone = stop_signal.clone();

        let handle = thread::spawn(move || {
            if let Err(e) = run_wasm_instance(&engine, &module, channel_clone, env_vars, log_channel, stop_clone) {
                eprintln!("[brrmmmm] WASM execution error: {e:?}");
            }
        });

        Ok(Self {
            channel_data,
            thread: Some(handle),
            stop_signal,
        })
    }

    /// Read the latest channel push, consuming it.
    pub fn poll_channel(&self) -> Option<Vec<u8>> {
        self.channel_data.lock().unwrap().take()
    }

    /// Signal the sidecar to stop and wait for the thread.
    pub fn stop(mut self) {
        *self.stop_signal.lock().unwrap() = true;
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for SidecarRunner {
    fn drop(&mut self) {
        *self.stop_signal.lock().unwrap() = true;
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

/// Run a single WASM instance with wasmtime, providing vzglyd_host imports.
fn run_wasm_instance(
    engine: &Engine,
    module: &Module,
    channel_data: Arc<Mutex<Option<Vec<u8>>>>,
    env_vars: Vec<(String, String)>,
    log_channel: bool,
    _stop_signal: Arc<Mutex<bool>>,
) -> Result<()> {
    // Build WASI preview1 context
    let mut wasi_builder = wasmtime_wasi::WasiCtxBuilder::new();
    for (key, value) in &env_vars {
        let _ = wasi_builder.env(key, value);
    }
    wasi_builder.inherit_stdout().inherit_stderr();
    let wasi_p1 = wasi_builder.build_p1();

    let mut store = Store::new(engine, wasi_p1);

    let mut linker: Linker<wasmtime_wasi::preview1::WasiP1Ctx> = Linker::new(engine);
    wasmtime_wasi::preview1::add_to_linker_sync(&mut linker, |ctx| ctx)?;

    // Build host state and register vzglyd_host imports
    let mut host_state = HostState::new(log_channel);
    host_state.channel_data.clone_from(&channel_data);
    register_vzglyd_host_on_linker(&mut linker, host_state)?;

    let instance = linker.instantiate(&mut store, module)
        .context("instantiate WASM module")?;

    // Look for _start or main as the entry point
    let entry = instance
        .get_func(&mut store, "_start")
        .or_else(|| instance.get_func(&mut store, "main"))
        .context("WASM module has no _start or main export")?;

    eprintln!("[brrmmmm] starting sidecar (entry: _start/main)...");
    eprintln!("[brrmmmm] sidecar poll_loop will run until stopped (Ctrl+C)");

    // Call the entry point — this runs poll_loop which never returns
    entry.call(&mut store, &[], &mut [])?;

    Ok(())
}

/// Register vzglyd_host imports on a Linker<wasmtime_wasi::preview1::WasiP1Ctx>.
/// Since the imports use HostState but the store uses wasmtime_wasi::preview1::WasiP1Ctx,
/// we store HostState inside wasmtime_wasi::preview1::WasiP1Ctx via Arc.
fn register_vzglyd_host_on_linker(linker: &mut Linker<wasmtime_wasi::preview1::WasiP1Ctx>, host_state: HostState) -> Result<()> {
    let shared = Arc::new(std::sync::Mutex::new(host_state));

    // Clone BEFORE each closure to avoid move-after-use
    let s1 = shared.clone();
    linker.func_wrap(
        "vzglyd_host",
        "channel_push",
        move |mut caller: wasmtime::Caller<'_, wasmtime_wasi::preview1::WasiP1Ctx>, ptr: i32, len: i32| -> i32 {
            let data = match read_memory_from_caller(&mut caller, ptr, len) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("[brrmmmm] channel_push memory error: {e}");
                    return -1;
                }
            };
            let guard = s1.lock().unwrap();
            if guard.log_channel {
                eprintln!("[brrmmmm] channel_push: {} bytes", data.len());
                if let Ok(s) = std::str::from_utf8(&data) {
                    eprintln!("[brrmmmm]   payload: {}", s.chars().take(200).collect::<String>());
                }
            }
            guard.channel_data.lock().unwrap().replace(data);
            0
        },
    )?;

    linker.func_wrap(
        "vzglyd_host",
        "channel_poll",
        |_caller: wasmtime::Caller<'_, wasmtime_wasi::preview1::WasiP1Ctx>, _ptr: i32, _len: i32| -> i32 { -1 },
    )?;

    linker.func_wrap(
        "vzglyd_host",
        "channel_active",
        |_caller: wasmtime::Caller<'_, wasmtime_wasi::preview1::WasiP1Ctx>| -> i32 { 1 },
    )?;

    // log_info doesn't need shared state, it just prints
    linker.func_wrap(
        "vzglyd_host",
        "log_info",
        move |mut caller: wasmtime::Caller<'_, wasmtime_wasi::preview1::WasiP1Ctx>, ptr: i32, len: i32| -> i32 {
            if let Ok(data) = read_memory_from_caller(&mut caller, ptr, len) {
                if let Ok(msg) = std::str::from_utf8(&data) {
                    eprintln!("[sidecar] {msg}");
                }
            }
            0
        },
    )?;

    // Network request/response
    let s_net = shared.clone();
    linker.func_wrap(
        "vzglyd_host",
        "network_request",
        move |mut caller: wasmtime::Caller<'_, wasmtime_wasi::preview1::WasiP1Ctx>, ptr: i32, len: i32| -> i32 {
            let req_bytes = match read_memory_from_caller(&mut caller, ptr, len) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("[brrmmmm] network_request memory error: {e}");
                    return -1;
                }
            };

            let decoded: serde_json::Value = match serde_json::from_slice(&req_bytes) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("[brrmmmm] network_request decode error: {e}");
                    return -1;
                }
            };

            let wire_version = decoded.get("wire_version").and_then(|v| v.as_u64()).unwrap_or(0) as u8;
            if wire_version != 1 {
                eprintln!("[brrmmmm] network_request wire_version mismatch: {wire_version}");
                return -1;
            }

            let request: crate::host::host_request::HostRequest = match serde_json::from_value(decoded.clone()) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("[brrmmmm] network_request parse error: {e}");
                    return -1;
                }
            };

            let response = match execute_native_request(&request) {
                Ok(resp) => resp,
                Err(e) => crate::host::host_request::HostResponse::Error {
                    error_kind: crate::host::host_request::ErrorKind::Io,
                    message: e,
                },
            };

            // Encode response with versioned envelope
            let resp_bytes = encode_response_for_sidecar(&response);
            eprintln!("[brrmmmm] network_request: {} response_len={}",
                match &response {
                    crate::host::host_request::HostResponse::Http { status_code, .. } => format!("http({status_code})"),
                    crate::host::host_request::HostResponse::TcpConnect { elapsed_ms } => format!("tcp({elapsed_ms}ms)"),
                    crate::host::host_request::HostResponse::Error { error_kind, message } => format!("error({error_kind:?}: {message})"),
                },
                resp_bytes.len()
            );

            let guard = s_net.lock().unwrap();
            *guard.pending_response.lock().unwrap() = Some(resp_bytes);
            0
        },
    )?;

    let s_resp = shared.clone();
    // note: s_resp defined above;
    linker.func_wrap(
        "vzglyd_host",
        "network_response_len",
        move |_caller: wasmtime::Caller<'_, wasmtime_wasi::preview1::WasiP1Ctx>| -> i32 {
            let guard = s_resp.lock().unwrap();
            guard.pending_response.lock().unwrap()
                .as_ref().map(|b| b.len() as i32).unwrap_or(-1)
        },
    )?;

    let shared_read = shared.clone();
    linker.func_wrap(
        "vzglyd_host",
        "network_response_read",
        move |mut caller: wasmtime::Caller<'_, wasmtime_wasi::preview1::WasiP1Ctx>, ptr: i32, len: i32| -> i32 {
            let guard = shared_read.lock().unwrap();
            let mut resp_guard = guard.pending_response.lock().unwrap();
            let Some(data) = resp_guard.take() else { return -1 };
            let write_len = std::cmp::min(data.len(), len as usize);
            if let Err(e) = write_memory_from_caller(&mut caller, ptr, &data[..write_len]) {
                eprintln!("[brrmmmm] network_response_read error: {e}");
                return -1;
            }
            write_len as i32
        },
    )?;

    // Tracing (no-op)
    linker.func_wrap(
        "vzglyd_host",
        "trace_span_start",
        |_caller: wasmtime::Caller<'_, wasmtime_wasi::preview1::WasiP1Ctx>, _ptr: i32, _len: i32| -> i32 { 1 },
    )?;

    linker.func_wrap(
        "vzglyd_host",
        "trace_span_end",
        |_caller: wasmtime::Caller<'_, wasmtime_wasi::preview1::WasiP1Ctx>, _span_id: i32, _ptr: i32, _len: i32| -> i32 { 0 },
    )?;

    linker.func_wrap(
        "vzglyd_host",
        "trace_event",
        |_caller: wasmtime::Caller<'_, wasmtime_wasi::preview1::WasiP1Ctx>, _ptr: i32, _len: i32| -> i32 { 0 },
    )?;

    Ok(())
}

// ── Memory helpers for Caller ────────────────────────────────────────

fn read_memory_from_caller(caller: &mut wasmtime::Caller<'_, wasmtime_wasi::preview1::WasiP1Ctx>, ptr: i32, len: i32) -> Result<Vec<u8>> {
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

fn write_memory_from_caller(caller: &mut wasmtime::Caller<'_, wasmtime_wasi::preview1::WasiP1Ctx>, ptr: i32, data: &[u8]) -> Result<()> {
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

// ── Response encoding for sidecar consumption ────────────────────────

fn encode_response_for_sidecar(response: &crate::host::host_request::HostResponse) -> Vec<u8> {
    use crate::host::host_request::{HostResponse, Header};
    match response {
        HostResponse::Http { status_code, headers, body } => {
            serde_json::json!({
                "wire_version": 1u8,
                "kind": "http",
                "status_code": status_code,
                "headers": headers.iter().map(|h| serde_json::json!({"name": h.name, "value": h.value})).collect::<Vec<_>>(),
                "body": body.iter().map(|&b| b as u64).collect::<Vec<_>>(),
            })
        }
        HostResponse::TcpConnect { elapsed_ms } => {
            serde_json::json!({
                "wire_version": 1u8,
                "kind": "tcp_connect",
                "elapsed_ms": elapsed_ms,
            })
        }
        HostResponse::Error { error_kind, message } => {
            serde_json::json!({
                "wire_version": 1u8,
                "kind": "error",
                "error_kind": format!("{error_kind:?}").to_lowercase(),
                "message": message,
            })
        }
    }.to_string().into_bytes()
}

// ── Native request execution ─────────────────────────────────────────

fn execute_native_request(req: &crate::host::host_request::HostRequest) -> Result<crate::host::host_request::HostResponse, String> {
    use crate::host::host_request::{HostRequest, HostResponse, Header, ErrorKind};

    match req {
        HostRequest::HttpsGet { host, path, headers } => {
            let url = format!("https://{host}{path}");
            eprintln!("[brrmmmm] GET {url}");

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
            let resp = client.get(&url).send().map_err(|e| format!("request: {e}"))?;
            let status_code = resp.status().as_u16();

            let resp_headers: Vec<Header> = resp.headers().iter()
                .filter_map(|(n, v)| Some(Header {
                    name: n.as_str().to_string(),
                    value: v.to_str().ok()?.to_string(),
                }))
                .collect();

            let body = resp.bytes().map_err(|e| format!("read body: {e}"))?.to_vec();

            Ok(HostResponse::Http {
                status_code,
                headers: resp_headers,
                body,
            })
        }
        HostRequest::TcpConnect { host, port, timeout_ms } => {
            let addr = format!("{host}:{port}");
            eprintln!("[brrmmmm] TCP connect {addr}");
            let start = std::time::Instant::now();
            let timeout = std::time::Duration::from_millis(*timeout_ms as u64);
            let _stream = std::net::TcpStream::connect_timeout(
                &addr.parse().map_err(|e| format!("parse addr: {e}"))?,
                timeout,
            ).map_err(|e| format!("connect: {e}"))?;
            Ok(HostResponse::TcpConnect {
                elapsed_ms: start.elapsed().as_millis() as u64,
            })
        }
    }
}
