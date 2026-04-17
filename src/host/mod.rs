pub mod host_request;

use std::sync::{Arc, Mutex};

// ── Artifact store ───────────────────────────────────────────────────

/// A received artifact with its raw bytes and receipt timestamp.
#[derive(Debug, Clone)]
pub struct Artifact {
    pub kind: String,
    pub data: Vec<u8>,
    #[allow(dead_code)]
    pub received_at_ms: u64,
}

/// Named artifact store replacing the old `channel_data` field.
///
/// v1 sidecars write to `published_output` via `channel_push`.
/// v2 sidecars use `artifact_publish` with an explicit kind.
#[derive(Debug, Default)]
pub struct ArtifactStore {
    pub raw_source: Option<Artifact>,
    pub normalized: Option<Artifact>,
    pub published_output: Option<Artifact>,
}

impl ArtifactStore {
    pub fn store(&mut self, artifact: Artifact) {
        match artifact.kind.as_str() {
            "raw_source_payload" => self.raw_source = Some(artifact),
            "normalized_payload" => self.normalized = Some(artifact),
            _ => self.published_output = Some(artifact), // "published_output" + v1 channel_push
        }
    }

    /// Take the published output (consumed once, like the old channel_data.take()).
    pub fn take_published(&mut self) -> Option<Artifact> {
        self.published_output.take()
    }
}

// ── Shared state between host imports and the runner ────────────────

/// State shared by all vzglyd_host import functions.
pub struct HostState {
    /// Named artifact store (replaces raw channel_data).
    pub artifact_store: Arc<Mutex<ArtifactStore>>,

    /// Pending response from a network_request, to be read by network_response_read.
    pub pending_response: Arc<Mutex<Option<Vec<u8>>>>,

    /// Whether to print channel pushes to stderr (--log-channel flag).
    pub log_channel: bool,

    /// Current JSON params made available to the sidecar through `params_len`/`params_read`.
    pub params_bytes: Arc<Mutex<Option<Vec<u8>>>>,
}

impl HostState {
    pub fn new(log_channel: bool, params_bytes: Arc<Mutex<Option<Vec<u8>>>>) -> Self {
        Self {
            artifact_store: Arc::new(Mutex::new(ArtifactStore::default())),
            pending_response: Arc::new(Mutex::new(None)),
            log_channel,
            params_bytes,
        }
    }
}
