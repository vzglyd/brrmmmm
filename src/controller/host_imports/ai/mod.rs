mod execute;

use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::attestation::RequestBinding;
use crate::events::{Event, EventSink, now_ts};
use crate::host::HostState;
use crate::host::ai_request::{AiAction, AiActionResponse};
use crate::host::host_call::{HostCallError, HostCallResult};
use crate::mission_state::{self, Capabilities};

use execute::AiSession;

pub(super) type SharedAiSession = Arc<AiSession>;

pub(super) fn new_session(config: &crate::config::Config) -> anyhow::Result<SharedAiSession> {
    Ok(Arc::new(AiSession::new(config)?))
}

pub(super) async fn handle(
    action: AiAction,
    shared: Arc<Mutex<HostState>>,
    event_sink: EventSink,
    session: SharedAiSession,
) -> HostCallResult {
    let action_kind = action.kind().to_string();
    let prompt_len = action.prompt_len();

    event_sink.emit(Event::AiRequest {
        ts: now_ts(),
        action: action_kind.clone(),
        prompt_len,
    });

    let body = session.prepare_body(&action);
    let body = match body {
        Ok(body) => body,
        Err(response) => {
            {
                let mut host = super::super::io::lock_runtime(&shared, "host_state");
                let event = mission_state::ai_event(&action_kind);
                host.record_activity(Capabilities::AI, "ai", &event);
            }
            emit_done(&event_sink, action_kind, 0, &response);
            return ai_response_to_result(response);
        }
    };

    let content_digest = crate::utils::sha256_digest(&body);
    let binding = RequestBinding::new(
        "POST",
        "api.anthropic.com",
        "/v1/messages",
        Some(content_digest),
    );
    let (ua, attestation_headers) = {
        let mut host = super::super::io::lock_runtime(&shared, "host_state");
        let event = mission_state::ai_event(&action_kind);
        let envelope = host.signed_envelope_for_request(Capabilities::AI, "ai", &event, &binding);
        let ua = host.full_user_agent(envelope.as_ref());
        let headers = envelope
            .map(|envelope| envelope.headers)
            .unwrap_or_default();
        (ua, headers)
    };

    let start = Instant::now();
    let response = session
        .execute_prepared(body, ua, attestation_headers)
        .await;
    let elapsed_ms = start.elapsed().as_millis() as u64;
    emit_done(&event_sink, action_kind, elapsed_ms, &response);
    ai_response_to_result(response)
}

fn emit_done(event_sink: &EventSink, action: String, elapsed_ms: u64, response: &AiActionResponse) {
    event_sink.emit(Event::AiRequestDone {
        ts: now_ts(),
        action,
        elapsed_ms,
        ok: response.is_ok(),
        error: response.error_code().map(ToOwned::to_owned),
    });
}

fn ai_response_to_result(response: AiActionResponse) -> HostCallResult {
    match response {
        AiActionResponse::Ok { text, .. } => Ok(serde_json::json!({ "text": text })),
        AiActionResponse::Err { error, message, .. } => Err(HostCallError::new(error, message)),
    }
}
