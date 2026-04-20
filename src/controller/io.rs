use std::sync::{Arc, Mutex, MutexGuard};

use crate::abi::{ArtifactMeta, SidecarPhase, SidecarRuntimeState};
use crate::events::{Event, EventSink, now_ms, now_ts};
use crate::host::host_request::{Header, HostRequest, HostResponse};

// ── Mutex helpers ────────────────────────────────────────────────────

pub(super) fn lock_runtime<'a, T>(mutex: &'a Mutex<T>, name: &str) -> MutexGuard<'a, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            eprintln!("[brrmmmm] recovering poisoned {name} mutex");
            poisoned.into_inner()
        }
    }
}

// ── Runtime state helpers ────────────────────────────────────────────

pub(super) fn update_phase_state(
    runtime_state: &Arc<Mutex<SidecarRuntimeState>>,
    event_sink: &EventSink,
    phase: SidecarPhase,
) {
    lock_runtime(runtime_state, "runtime_state").phase = phase.clone();
    event_sink.emit(Event::Phase {
        ts: now_ts(),
        phase,
    });
}

pub(super) fn update_sleep_state(
    runtime_state: &Arc<Mutex<SidecarRuntimeState>>,
    event_sink: &EventSink,
    duration_ms: u64,
    wake_ms: u64,
) {
    {
        let mut state = lock_runtime(runtime_state, "runtime_state");
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

pub(super) fn update_artifact_state(
    runtime_state: &Arc<Mutex<SidecarRuntimeState>>,
    meta: &ArtifactMeta,
) {
    let mut state = lock_runtime(runtime_state, "runtime_state");
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

pub(super) fn update_failure_state(runtime_state: &Arc<Mutex<SidecarRuntimeState>>, error: &str) {
    let mut state = lock_runtime(runtime_state, "runtime_state");
    state.phase = SidecarPhase::Failed;
    state.last_failure_at_ms = Some(now_ms());
    state.consecutive_failures = state.consecutive_failures.saturating_add(1);
    state.last_error = Some(error.to_string());
}

// ── Request helpers ──────────────────────────────────────────────────

pub(super) fn describe_request(req: &HostRequest) -> (String, String, Option<String>) {
    match req {
        HostRequest::HttpsGet { host, path, .. } => {
            ("https_get".to_string(), host.clone(), Some(path.clone()))
        }
        HostRequest::TcpConnect { host, port, .. } => {
            ("tcp_connect".to_string(), format!("{host}:{port}"), None)
        }
    }
}

pub(super) fn response_info(resp: &HostResponse) -> (Option<u16>, usize) {
    match resp {
        HostResponse::Http {
            status_code, body, ..
        } => (Some(*status_code), body.len()),
        HostResponse::TcpConnect { .. } => (None, 0),
        HostResponse::Error { .. } => (None, 0),
    }
}

// ── Memory helpers ───────────────────────────────────────────────────

pub(super) fn read_memory_from_caller(
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

pub(super) fn write_memory_from_caller(
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

pub(super) fn encode_response_for_sidecar(response: &HostResponse) -> Vec<u8> {
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

pub(super) fn execute_native_request(
    req: &HostRequest,
    user_agent: &str,
) -> Result<HostResponse, String> {
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

            let mut hm = reqwest::header::HeaderMap::new();
            if let Ok(v) = reqwest::header::HeaderValue::from_str(user_agent) {
                hm.insert(reqwest::header::USER_AGENT, v);
            }
            for h in headers {
                if let (Ok(n), Ok(v)) = (
                    reqwest::header::HeaderName::from_bytes(h.name.as_bytes()),
                    reqwest::header::HeaderValue::from_bytes(h.value.as_bytes()),
                ) {
                    hm.insert(n, v);
                }
            }
            builder = builder.default_headers(hm);

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
