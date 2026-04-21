use std::sync::{Arc, Mutex};

use base64::Engine as _;

use crate::abi::{ArtifactMeta, SidecarRuntimeState};
use crate::events::{Event, EventSink, now_ms, now_ts};
use crate::host::host_request::NetworkResponseData;
use crate::host::{Artifact, HostState};

use super::super::super::io::lock_runtime;
use super::super::super::io::update_artifact_state;

pub(super) fn publish_raw_source_payload(
    response: &NetworkResponseData,
    shared: &Arc<Mutex<HostState>>,
    runtime_state: &Arc<Mutex<SidecarRuntimeState>>,
    sink: &EventSink,
) {
    let NetworkResponseData::Http {
        status_code,
        body_base64,
        ..
    } = response
    else {
        return;
    };

    if *status_code >= 400 {
        return;
    }

    let Ok(body) = base64::engine::general_purpose::STANDARD.decode(body_base64) else {
        return;
    };

    let received_at_ms = now_ms();
    let preview_chars = lock_runtime(shared, "host_state")
        .config
        .limits
        .max_artifact_preview_chars;
    let preview = String::from_utf8_lossy(&body)
        .chars()
        .take(preview_chars)
        .collect();
    let artifact = Artifact {
        kind: "raw_source_payload".to_string(),
        data: body.clone(),
        received_at_ms,
    };
    let artifact_store = {
        let host = lock_runtime(shared, "host_state");
        Arc::clone(&host.artifact_store)
    };
    lock_runtime(&artifact_store, "artifact_store").store(artifact);

    let meta = ArtifactMeta {
        kind: "raw_source_payload".to_string(),
        size_bytes: body.len(),
        received_at_ms,
    };
    update_artifact_state(runtime_state, &meta);
    sink.emit(Event::ArtifactReceived {
        ts: now_ts(),
        kind: "raw_source_payload".to_string(),
        size_bytes: body.len(),
        preview,
        artifact: meta,
    });
}
