use std::sync::{Arc, Mutex};

use crate::host::{Artifact, HostState};

use super::super::super::io::lock_runtime;

pub(super) fn store_pending_response(shared: &Arc<Mutex<HostState>>, data: Vec<u8>) {
    let pending_response = pending_response_handle(shared);
    *lock_runtime(&pending_response, "pending_response") = Some(data);
}

pub(super) fn pending_response_len(shared: &Arc<Mutex<HostState>>) -> i32 {
    let pending_response = pending_response_handle(shared);
    lock_runtime(&pending_response, "pending_response")
        .as_ref()
        .map(|bytes| bytes.len() as i32)
        .unwrap_or(-1)
}

pub(super) fn take_pending_response(shared: &Arc<Mutex<HostState>>) -> Option<Vec<u8>> {
    let pending_response = pending_response_handle(shared);
    lock_runtime(&pending_response, "pending_response").take()
}

pub(super) fn store_artifact(shared: &Arc<Mutex<HostState>>, artifact: Artifact) {
    let artifact_store = {
        let host = lock_runtime(shared, "host_state");
        Arc::clone(&host.artifact_store)
    };
    lock_runtime(&artifact_store, "artifact_store").store(artifact);
}

fn pending_response_handle(shared: &Arc<Mutex<HostState>>) -> Arc<Mutex<Option<Vec<u8>>>> {
    let host = lock_runtime(shared, "host_state");
    Arc::clone(&host.pending_response)
}
