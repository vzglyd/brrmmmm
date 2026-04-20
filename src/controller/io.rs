use std::io::Read as _;
use std::sync::{Arc, Mutex, MutexGuard};

use wasmtime::{Caller, Engine, Linker, Store, StoreLimits, StoreLimitsBuilder};

use crate::abi::{ArtifactMeta, SidecarPhase, SidecarRuntimeState};
use crate::attestation;
use crate::config::RuntimeLimits;
use crate::events::{Event, EventSink, diag, now_ms, now_ts};
use crate::host::host_request::{ErrorKind, Header, HostRequest, HostResponse};

// ── WASM runtime policy and store types ──────────────────────────────

pub(super) const EPOCH_TICKS_PER_SECOND: u64 = 10;

#[derive(Debug, Clone)]
pub(super) struct RuntimePolicy {
    pub(super) init_timeout_secs: u64,
    pub(super) default_acquisition_timeout_secs: u64,
    pub(super) max_wasm_memory_bytes: usize,
    pub(super) max_table_elements: usize,
    pub(super) max_instances: usize,
    pub(super) max_memories: usize,
    pub(super) max_tables: usize,
    pub(super) max_params_bytes: usize,
    pub(super) max_describe_bytes: usize,
    pub(super) legacy_configure_buffer: bool,
}

impl Default for RuntimePolicy {
    fn default() -> Self {
        Self {
            init_timeout_secs: 60,
            default_acquisition_timeout_secs: 30,
            max_wasm_memory_bytes: 128 * 1024 * 1024,
            max_table_elements: 1_000_000,
            max_instances: 4,
            max_memories: 4,
            max_tables: 8,
            max_params_bytes: 1024 * 1024,
            max_describe_bytes: 1024 * 1024,
            legacy_configure_buffer: false,
        }
    }
}

impl RuntimePolicy {
    pub(super) fn from_limits(limits: &RuntimeLimits) -> Self {
        Self {
            max_params_bytes: limits.max_params_bytes,
            max_describe_bytes: limits.max_host_payload_bytes,
            ..Self::default()
        }
    }

    pub(super) fn epoch_ticks_for_secs(&self, secs: u64) -> u64 {
        secs.saturating_mul(EPOCH_TICKS_PER_SECOND).max(1)
    }

    pub(super) fn init_deadline_ticks(&self) -> u64 {
        self.epoch_ticks_for_secs(self.init_timeout_secs)
    }

    pub(super) fn acquisition_deadline_ticks(&self, timeout_secs: u64) -> u64 {
        self.epoch_ticks_for_secs(timeout_secs)
    }

    fn store_limits(&self) -> StoreLimits {
        StoreLimitsBuilder::new()
            .memory_size(self.max_wasm_memory_bytes)
            .table_elements(self.max_table_elements)
            .instances(self.max_instances)
            .memories(self.max_memories)
            .tables(self.max_tables)
            .trap_on_grow_failure(true)
            .build()
    }
}

pub(super) struct WasmStoreState {
    pub(super) wasi: wasmtime_wasi::preview1::WasiP1Ctx,
    limits: StoreLimits,
}

impl WasmStoreState {
    fn new(wasi: wasmtime_wasi::preview1::WasiP1Ctx, policy: &RuntimePolicy) -> Self {
        Self {
            wasi,
            limits: policy.store_limits(),
        }
    }
}

pub(super) type WasmCaller<'a> = Caller<'a, WasmStoreState>;
pub(super) type WasmLinker = Linker<WasmStoreState>;
pub(super) type WasmStore = Store<WasmStoreState>;

pub(super) fn build_wasm_store(
    engine: &Engine,
    wasi: wasmtime_wasi::preview1::WasiP1Ctx,
    policy: &RuntimePolicy,
) -> WasmStore {
    let mut store = Store::new(engine, WasmStoreState::new(wasi, policy));
    store.limiter(|state| &mut state.limits);
    store
}

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
    let previous = {
        let mut state = lock_runtime(runtime_state, "runtime_state");
        let prev = state.phase.clone();
        if !is_valid_phase_transition(&prev, &phase) {
            diag(
                event_sink,
                &format!("[brrmmmm] rejected invalid phase transition {prev:?} -> {phase:?}"),
            );
            return;
        }
        state.phase = phase.clone();
        prev
    };
    tracing::trace!(from = ?previous, to = ?phase, "sidecar phase transition");
    event_sink.emit(Event::Phase {
        ts: now_ts(),
        phase,
    });
}

fn is_valid_phase_transition(from: &SidecarPhase, to: &SidecarPhase) -> bool {
    use SidecarPhase::*;
    // Self-transitions are always valid (e.g. multiple artifact publishes in sequence).
    if from == to {
        return true;
    }
    // Any phase may transition to Failed or CoolingDown.
    if matches!(to, Failed | CoolingDown) {
        return true;
    }
    matches!(
        (from, to),
        (Idle, Fetching)
            | (Idle, Publishing)
            | (CoolingDown, Fetching)
            | (CoolingDown, Idle)
            | (Failed, Fetching)
            | (Failed, Idle)
            | (Fetching, Parsing)
            | (Fetching, Publishing)
            | (Parsing, Publishing)
            | (Publishing, Idle)
    )
}

pub(super) fn update_sleep_state(
    runtime_state: &Arc<Mutex<SidecarRuntimeState>>,
    event_sink: &EventSink,
    duration_ms: u64,
    wake_ms: u64,
) {
    {
        let mut state = lock_runtime(runtime_state, "runtime_state");
        if !is_valid_phase_transition(&state.phase, &SidecarPhase::CoolingDown) {
            let previous = state.phase.clone();
            drop(state);
            diag(
                event_sink,
                &format!(
                    "[brrmmmm] rejected invalid phase transition {previous:?} -> {:?}",
                    SidecarPhase::CoolingDown
                ),
            );
            return;
        }
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
    caller: &mut WasmCaller<'_>,
    ptr: i32,
    len: i32,
) -> anyhow::Result<Vec<u8>> {
    if ptr < 0 || len < 0 {
        anyhow::bail!("memory read invalid negative range: ptr={ptr}, len={len}");
    }
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

pub(super) fn read_limited_memory_from_caller(
    caller: &mut WasmCaller<'_>,
    ptr: i32,
    len: i32,
    limit: usize,
    label: &str,
) -> anyhow::Result<Vec<u8>> {
    if len < 0 {
        anyhow::bail!("{label} length is negative: {len}");
    }
    let len = len as usize;
    if len > limit {
        anyhow::bail!("{label} length {len} exceeds configured limit of {limit} bytes");
    }
    read_memory_from_caller(caller, ptr, len as i32)
}

pub(super) fn write_memory_from_caller(
    caller: &mut WasmCaller<'_>,
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
    attestation_headers: &[(String, String)],
    max_response_bytes: usize,
) -> Result<HostResponse, (ErrorKind, String)> {
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
            for h in headers {
                if attestation::is_reserved_header(&h.name) {
                    continue;
                }
                if let (Ok(n), Ok(v)) = (
                    reqwest::header::HeaderName::from_bytes(h.name.as_bytes()),
                    reqwest::header::HeaderValue::from_bytes(h.value.as_bytes()),
                ) {
                    hm.insert(n, v);
                }
            }
            if let Ok(v) = reqwest::header::HeaderValue::from_str(user_agent) {
                hm.insert(reqwest::header::USER_AGENT, v);
            }
            for (name, value) in attestation_headers {
                if let (Ok(n), Ok(v)) = (
                    reqwest::header::HeaderName::from_bytes(name.as_bytes()),
                    reqwest::header::HeaderValue::from_str(value),
                ) {
                    hm.insert(n, v);
                }
            }
            builder = builder.default_headers(hm);

            let client = builder
                .build()
                .map_err(|e| (ErrorKind::Io, format!("build client: {e}")))?;
            let resp = client
                .get(&url)
                .send()
                .map_err(|e| classify_reqwest_error(&e, format!("request: {e}")))?;
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

            if let Some(content_length) = resp.content_length()
                && content_length > max_response_bytes as u64
            {
                return Err((
                    ErrorKind::Io,
                    format!(
                        "response body is {content_length} bytes, exceeding configured limit of {max_response_bytes} bytes"
                    ),
                ));
            }

            let mut reader = resp.take(max_response_bytes.saturating_add(1) as u64);
            let mut body = Vec::new();
            std::io::Read::read_to_end(&mut reader, &mut body)
                .map_err(|e| (ErrorKind::Io, format!("read body: {e}")))?;
            if body.len() > max_response_bytes {
                return Err((
                    ErrorKind::Io,
                    format!("response body exceeds configured limit of {max_response_bytes} bytes"),
                ));
            }

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
                &addr
                    .parse()
                    .map_err(|e| (ErrorKind::Io, format!("parse addr: {e}")))?,
                timeout,
            )
            .map_err(|e| classify_io_error(&e, format!("connect: {e}")))?;
            Ok(HostResponse::TcpConnect {
                elapsed_ms: start.elapsed().as_millis() as u64,
            })
        }
    }
}

fn classify_reqwest_error(e: &reqwest::Error, message: String) -> (ErrorKind, String) {
    if e.is_timeout() {
        return (ErrorKind::Timeout, message);
    }
    if e.is_connect()
        && let Some(source) = std::error::Error::source(e)
        && let Some(io_err) = source.downcast_ref::<std::io::Error>()
    {
        let kind = io_kind_to_error_kind(io_err.kind());
        if kind != ErrorKind::Io {
            return (kind, message);
        }
    }
    // reqwest surfaces DNS failures as "builder" errors in some configurations;
    // check the message as a heuristic.
    let msg_lower = message.to_ascii_lowercase();
    if msg_lower.contains("dns") || msg_lower.contains("resolve") || msg_lower.contains("lookup") {
        return (ErrorKind::Dns, message);
    }
    if msg_lower.contains("tls") || msg_lower.contains("ssl") || msg_lower.contains("certificate") {
        return (ErrorKind::Tls, message);
    }
    (ErrorKind::Io, message)
}

fn classify_io_error(e: &std::io::Error, message: String) -> (ErrorKind, String) {
    (io_kind_to_error_kind(e.kind()), message)
}

fn io_kind_to_error_kind(k: std::io::ErrorKind) -> ErrorKind {
    match k {
        std::io::ErrorKind::ConnectionRefused => ErrorKind::ConnectionRefused,
        std::io::ErrorKind::PermissionDenied => ErrorKind::PermissionDenied,
        std::io::ErrorKind::TimedOut => ErrorKind::Timeout,
        _ => ErrorKind::Io,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_transition_allows_expected_lifecycle() {
        assert!(is_valid_phase_transition(
            &SidecarPhase::Idle,
            &SidecarPhase::Fetching
        ));
        assert!(is_valid_phase_transition(
            &SidecarPhase::Fetching,
            &SidecarPhase::Parsing
        ));
        assert!(is_valid_phase_transition(
            &SidecarPhase::Parsing,
            &SidecarPhase::Publishing
        ));
        assert!(is_valid_phase_transition(
            &SidecarPhase::Publishing,
            &SidecarPhase::Idle
        ));
    }

    #[test]
    fn phase_transition_rejects_invalid_jump() {
        assert!(!is_valid_phase_transition(
            &SidecarPhase::Idle,
            &SidecarPhase::Parsing
        ));
        assert!(!is_valid_phase_transition(
            &SidecarPhase::Parsing,
            &SidecarPhase::Fetching
        ));
    }
}
