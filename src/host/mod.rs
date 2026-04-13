pub mod host_request;

use std::sync::{Arc, Mutex};

// ── Shared state between host imports and the runner ────────────────

/// State shared by all vzglyd_host import functions.
pub struct HostState {
    /// Latest payload pushed by the sidecar via channel_push.
    pub channel_data: Arc<Mutex<Option<Vec<u8>>>>,

    /// Pending response from a network_request, to be read by network_response_read.
    pub pending_response: Arc<Mutex<Option<Vec<u8>>>>,

    /// Whether to print channel pushes to stderr.
    pub log_channel: bool,
}

impl HostState {
    pub fn new(log_channel: bool) -> Self {
        Self {
            channel_data: Arc::new(Mutex::new(None)),
            pending_response: Arc::new(Mutex::new(None)),
            log_channel,
        }
    }
}
