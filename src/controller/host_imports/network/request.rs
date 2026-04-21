use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

use base64::Engine as _;

use crate::abi::{MissionPhase, MissionRuntimeState};
use crate::attestation;
use crate::events::{Event, EventSink, now_ts};
use crate::host::HostState;
use crate::host::host_call::{HostCallError, HostCallResult};
use crate::host::host_request::{ErrorKind, Header, NetworkAction, NetworkResponseData};
use crate::mission_state::{self, Capabilities};

use super::super::super::io::{
    classify_io_error, classify_reqwest_error, lock_runtime, update_failure_state,
    update_phase_state,
};
use super::publish::publish_raw_source_payload;

pub struct NetworkSession {
    client: reqwest::Client,
}

struct HttpExecutionRequest {
    method: String,
    url: String,
    headers: Vec<Header>,
    body_base64: Option<String>,
    timeout_ms: u32,
    max_response_bytes: usize,
}

impl NetworkSession {
    pub(crate) fn new() -> anyhow::Result<Self> {
        let client = reqwest::Client::builder().use_rustls_tls().build()?;
        Ok(Self { client })
    }

    pub(crate) async fn execute(
        &self,
        action: NetworkAction,
        shared: Arc<Mutex<HostState>>,
        event_sink: EventSink,
        runtime_state: Arc<Mutex<MissionRuntimeState>>,
        request_counter: Arc<AtomicU64>,
    ) -> HostCallResult {
        let limits = lock_runtime(&shared, "host_state").config.limits.clone();
        let req_id = request_counter.fetch_add(1, Ordering::Relaxed);
        let request_id = format!("r{req_id}");
        let description = describe_action(&action);

        update_phase_state(&runtime_state, &event_sink, MissionPhase::Fetching);
        event_sink.emit(&Event::RequestStart {
            ts: now_ts(),
            request_id: request_id.clone(),
            kind: description.kind,
            host: description.host,
            path: description.path,
        });

        let start = Instant::now();
        let response = match self
            .execute_inner(action, shared.clone(), limits.max_http_response_bytes)
            .await
        {
            Ok(response) => response,
            Err(error) => {
                update_failure_state(&runtime_state, &error.message);
                event_sink.emit(&Event::RequestError {
                    ts: now_ts(),
                    request_id,
                    error_kind: error.kind.clone(),
                    message: error.message.clone(),
                });
                return Err(error);
            }
        };

        let elapsed_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
        let (status_code, response_size) = response_info(&response);
        event_sink.emit(&Event::RequestDone {
            ts: now_ts(),
            request_id,
            status_code,
            elapsed_ms,
            response_size_bytes: response_size,
        });

        publish_raw_source_payload(&response, &shared, &runtime_state, &event_sink);
        serde_json::to_value(response).map_err(|error| {
            HostCallError::new("encode_error", format!("encode network response: {error}"))
        })
    }

    async fn execute_inner(
        &self,
        action: NetworkAction,
        shared: Arc<Mutex<HostState>>,
        max_response_bytes: usize,
    ) -> Result<NetworkResponseData, HostCallError> {
        match action {
            NetworkAction::Http {
                method,
                url,
                headers,
                body_base64,
                timeout_ms,
            } => {
                self.execute_http(
                    HttpExecutionRequest {
                        method,
                        url,
                        headers,
                        body_base64,
                        timeout_ms,
                        max_response_bytes,
                    },
                    shared,
                )
                .await
            }
            NetworkAction::TcpConnect {
                host,
                port,
                timeout_ms,
            } => {
                self.execute_tcp_connect(host, port, timeout_ms, shared)
                    .await
            }
        }
    }

    async fn execute_http(
        &self,
        request: HttpExecutionRequest,
        shared: Arc<Mutex<HostState>>,
    ) -> Result<NetworkResponseData, HostCallError> {
        let HttpExecutionRequest {
            method,
            url,
            headers,
            body_base64,
            timeout_ms,
            max_response_bytes,
        } = request;

        let parsed_url = reqwest::Url::parse(&url)
            .map_err(|error| HostCallError::new("invalid_request", error.to_string()))?;
        let method = reqwest::Method::from_bytes(method.as_bytes())
            .map_err(|error| HostCallError::new("invalid_request", error.to_string()))?;
        let body = decode_request_body(body_base64)?;

        let content_digest = body.as_ref().map(|body| crate::utils::sha256_digest(body));
        let binding =
            attestation::binding_from_url(method.as_str(), parsed_url.as_str(), content_digest);
        let (user_agent, attestation_headers) =
            http_headers_for_request(&shared, &method, &parsed_url, binding.as_ref());

        let mut request_builder = self
            .client
            .request(method, parsed_url)
            .timeout(Duration::from_millis(u64::from(timeout_ms)))
            .headers(merge_header_map(headers, &user_agent, attestation_headers));
        if let Some(body) = body {
            request_builder = request_builder.body(body);
        }

        let response = request_builder.send().await.map_err(|error| {
            let (kind, message) = classify_reqwest_error(&error, format!("request: {error}"));
            HostCallError::new(kind.as_str(), message)
        })?;
        let status_code = response.status().as_u16();
        let response_headers = collect_response_headers(&response);

        if let Some(content_length) = response.content_length()
            && content_length > max_response_bytes as u64
        {
            return Err(HostCallError::new(
                ErrorKind::Io.as_str(),
                format!(
                    "response body is {content_length} bytes, exceeding configured limit of {max_response_bytes} bytes"
                ),
            ));
        }

        let body = read_capped_body(response, max_response_bytes).await?;
        Ok(NetworkResponseData::Http {
            status_code,
            headers: response_headers,
            body_base64: base64::engine::general_purpose::STANDARD.encode(body),
        })
    }

    async fn execute_tcp_connect(
        &self,
        host: String,
        port: u16,
        timeout_ms: u32,
        shared: Arc<Mutex<HostState>>,
    ) -> Result<NetworkResponseData, HostCallError> {
        let event = format!("TCP\n{}:{}", host.to_ascii_lowercase(), port).into_bytes();
        {
            let mut state = lock_runtime(&shared, "host_state");
            state.record_activity(Capabilities::NETWORK, "network", &event);
        }
        let addr = format!("{host}:{port}");
        let timeout = Duration::from_millis(u64::from(timeout_ms));
        let start = Instant::now();
        let connect = tokio::time::timeout(timeout, tokio::net::TcpStream::connect(&addr))
            .await
            .map_err(|_| {
                HostCallError::new(
                    ErrorKind::Timeout.as_str(),
                    format!("connect timeout after {timeout_ms}ms"),
                )
            })?;
        connect.map_err(|error| {
            let (kind, message) = classify_io_error(&error, format!("connect: {error}"));
            HostCallError::new(kind.as_str(), message)
        })?;
        Ok(NetworkResponseData::TcpConnect {
            elapsed_ms: u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
        })
    }
}

struct ActionDescription {
    kind: String,
    host: String,
    path: Option<String>,
}

fn decode_request_body(body_base64: Option<String>) -> Result<Option<Vec<u8>>, HostCallError> {
    body_base64
        .map(|body| {
            base64::engine::general_purpose::STANDARD
                .decode(body)
                .map_err(|error| HostCallError::new("invalid_request", error.to_string()))
        })
        .transpose()
}

fn http_headers_for_request(
    shared: &Arc<Mutex<HostState>>,
    method: &reqwest::Method,
    parsed_url: &reqwest::Url,
    binding: Option<&attestation::RequestBinding>,
) -> (String, Vec<(String, String)>) {
    let mut host = lock_runtime(shared, "host_state");
    let envelope = if let Some(binding) = binding {
        let event =
            mission_state::network_event(&binding.method, &binding.authority, &binding.path);
        host.signed_envelope_for_request(Capabilities::NETWORK, "network", &event, binding)
    } else {
        let event = mission_state::network_event(
            method.as_str(),
            parsed_url.host_str().unwrap_or_default(),
            parsed_url.path(),
        );
        host.record_activity(Capabilities::NETWORK, "network", &event);
        None
    };
    let user_agent = host.full_user_agent(envelope.as_ref());
    drop(host);
    let headers = envelope
        .map(|envelope| envelope.headers)
        .unwrap_or_default();
    (user_agent, headers)
}

fn merge_header_map(
    headers: Vec<Header>,
    user_agent: &str,
    attestation_headers: Vec<(String, String)>,
) -> reqwest::header::HeaderMap {
    let mut header_map = reqwest::header::HeaderMap::new();
    for header in headers {
        if crate::attestation::is_reserved_header(&header.name) {
            continue;
        }
        if let (Ok(name), Ok(value)) = (
            reqwest::header::HeaderName::from_bytes(header.name.as_bytes()),
            reqwest::header::HeaderValue::from_bytes(header.value.as_bytes()),
        ) {
            header_map.insert(name, value);
        }
    }
    if let Ok(value) = reqwest::header::HeaderValue::from_str(user_agent) {
        header_map.insert(reqwest::header::USER_AGENT, value);
    }
    for (name, value) in attestation_headers {
        if let (Ok(name), Ok(value)) = (
            reqwest::header::HeaderName::from_bytes(name.as_bytes()),
            reqwest::header::HeaderValue::from_str(&value),
        ) {
            header_map.insert(name, value);
        }
    }
    header_map
}

fn collect_response_headers(response: &reqwest::Response) -> Vec<Header> {
    response
        .headers()
        .iter()
        .filter_map(|(name, value)| {
            Some(Header {
                name: name.as_str().to_string(),
                value: value.to_str().ok()?.to_string(),
            })
        })
        .collect()
}

fn describe_action(action: &NetworkAction) -> ActionDescription {
    match action {
        NetworkAction::Http { method, url, .. } => reqwest::Url::parse(url).map_or_else(
            |_| ActionDescription {
                kind: "http".to_string(),
                host: String::new(),
                path: Some(url.clone()),
            },
            |parsed| ActionDescription {
                kind: format!("http_{}", method.to_ascii_lowercase()),
                host: parsed.host_str().unwrap_or_default().to_string(),
                path: Some(parsed.path().to_string()),
            },
        ),
        NetworkAction::TcpConnect { host, port, .. } => ActionDescription {
            kind: "tcp_connect".to_string(),
            host: host.clone(),
            path: Some(port.to_string()),
        },
    }
}

fn response_info(resp: &NetworkResponseData) -> (Option<u16>, usize) {
    match resp {
        NetworkResponseData::Http {
            status_code,
            body_base64,
            ..
        } => (
            Some(*status_code),
            base64::engine::general_purpose::STANDARD
                .decode(body_base64)
                .map(|body| body.len())
                .unwrap_or(0),
        ),
        NetworkResponseData::TcpConnect { .. } => (None, 0),
    }
}

async fn read_capped_body(
    mut response: reqwest::Response,
    max_response_bytes: usize,
) -> Result<Vec<u8>, HostCallError> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| {
        let (kind, message) = classify_reqwest_error(&error, format!("read body: {error}"));
        HostCallError::new(kind.as_str(), message)
    })? {
        if body.len().saturating_add(chunk.len()) > max_response_bytes {
            return Err(HostCallError::new(
                ErrorKind::Io.as_str(),
                format!("response body exceeds configured limit of {max_response_bytes} bytes"),
            ));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}
