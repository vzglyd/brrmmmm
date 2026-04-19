use std::sync::{Arc, Mutex};

use crate::host::HostState;

use super::super::super::io::lock_runtime;

pub(super) fn store_pending_kv_response(shared: &Arc<Mutex<HostState>>, data: Vec<u8>) {
    let pending_response = pending_kv_response_handle(shared);
    *lock_runtime(&pending_response, "pending_kv_response") = Some(data);
}

pub(super) fn clear_pending_kv_response(shared: &Arc<Mutex<HostState>>) {
    let pending_response = pending_kv_response_handle(shared);
    *lock_runtime(&pending_response, "pending_kv_response") = None;
}

pub(super) fn pending_kv_response_len(shared: &Arc<Mutex<HostState>>) -> i32 {
    let pending_response = pending_kv_response_handle(shared);
    lock_runtime(&pending_response, "pending_kv_response")
        .as_ref()
        .map(|bytes| bytes.len() as i32)
        .unwrap_or(-1)
}

pub(super) fn take_pending_kv_response(shared: &Arc<Mutex<HostState>>) -> Option<Vec<u8>> {
    let pending_response = pending_kv_response_handle(shared);
    lock_runtime(&pending_response, "pending_kv_response").take()
}

fn pending_kv_response_handle(shared: &Arc<Mutex<HostState>>) -> Arc<Mutex<Option<Vec<u8>>>> {
    let host = lock_runtime(shared, "host_state");
    Arc::clone(&host.pending_kv_response)
}
