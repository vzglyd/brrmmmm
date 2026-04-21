mod execute;

use std::sync::{Arc, Mutex};

use tokio::sync::Mutex as AsyncMutex;

use crate::events::EventSink;
use crate::host::HostState;
use crate::host::browser_request::{BrowserAction, BrowserActionResponse};
use crate::host::host_call::{HostCallError, HostCallResult};
use crate::mission_state::{self, Capabilities};

use execute::BrowserSession;

pub(super) type SharedBrowserSession = Arc<AsyncMutex<BrowserSession>>;

pub(super) fn new_session(shared: Arc<Mutex<HostState>>) -> SharedBrowserSession {
    Arc::new(AsyncMutex::new(BrowserSession::new(shared)))
}

pub(super) async fn handle(
    action: BrowserAction,
    shared: Arc<Mutex<HostState>>,
    event_sink: EventSink,
    session: SharedBrowserSession,
) -> HostCallResult {
    let action_kind = action.kind().to_string();
    let action_detail = action.detail();
    {
        let mut host = super::super::io::lock_runtime(&shared, "host_state");
        let event = mission_state::browser_action_event(&action_kind);
        host.record_activity(Capabilities::BROWSER, "browser_action", &event);
    }

    event_sink.emit(&crate::events::Event::BrowserAction {
        ts: crate::events::now_ts(),
        action: action_kind.clone(),
        detail: action_detail,
    });

    let start = std::time::Instant::now();
    let response = {
        let mut sess = session.lock().await;
        sess.execute(action).await
    };
    let elapsed_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
    let ok = response.is_ok();
    let error = if ok { None } else { Some(action_kind.clone()) };

    event_sink.emit(&crate::events::Event::BrowserActionDone {
        ts: crate::events::now_ts(),
        action: action_kind,
        elapsed_ms,
        ok,
        error,
    });

    browser_response_to_result(response)
}

fn browser_response_to_result(response: BrowserActionResponse) -> HostCallResult {
    match response {
        BrowserActionResponse::Ok { .. } => Ok(serde_json::json!({})),
        BrowserActionResponse::OkUrl { url, .. } => Ok(serde_json::json!({ "url": url })),
        BrowserActionResponse::OkCookies { cookies, .. } => {
            serde_json::to_value(serde_json::json!({ "cookies": cookies }))
                .map_err(|error| json_error(&error))
        }
        BrowserActionResponse::OkText { texts, .. } => Ok(serde_json::json!({ "texts": texts })),
        BrowserActionResponse::OkHtml { html, count, .. } => {
            Ok(serde_json::json!({ "html": html, "count": count }))
        }
        BrowserActionResponse::OkJson { value, .. } => Ok(serde_json::json!({ "value": value })),
        BrowserActionResponse::OkScreenshot { png_b64, .. } => {
            Ok(serde_json::json!({ "png_b64": png_b64 }))
        }
        BrowserActionResponse::Err { error, message, .. } => {
            Err(HostCallError::new(error, message))
        }
    }
}

fn json_error(error: &serde_json::Error) -> HostCallError {
    HostCallError::new("encode_error", error.to_string())
}
