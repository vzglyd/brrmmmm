pub mod ai_request;
pub mod browser_request;
pub mod host_request;

use std::sync::{Arc, Mutex};

use crate::abi::SidecarDescribe;
use crate::attestation::{self, EnvelopeFields, RequestBinding, SignedEnvelope};
use crate::events::now_ms;
use crate::identity::InstallationIdentity;
use crate::mission_state::MissionState;

// ── Artifact store ───────────────────────────────────────────────────

/// A received artifact with its raw bytes and receipt timestamp.
#[derive(Debug, Clone)]
pub struct Artifact {
    pub kind: String,
    pub data: Vec<u8>,
    #[allow(dead_code)]
    pub received_at_ms: u64,
}

/// Named artifact store for sidecar outputs.
///
/// Sidecars write `published_output` via `channel_push` or `artifact_publish` with an explicit kind.
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

    pub fn clear(&mut self) {
        self.raw_source = None;
        self.normalized = None;
        self.published_output = None;
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

    /// Pending response from a browser_action, to be read by browser_response_read.
    pub pending_browser_response: Arc<Mutex<Option<Vec<u8>>>>,

    /// Pending response from an ai_request, to be read by ai_response_read.
    pub pending_ai_response: Arc<Mutex<Option<Vec<u8>>>>,

    /// Pending value from a kv_get, to be read by kv_response_read.
    pub pending_kv_response: Arc<Mutex<Option<Vec<u8>>>>,

    /// Whether to print channel pushes to stderr (--log-channel flag).
    pub log_channel: bool,

    /// Current JSON params made available to the sidecar through `params_len`/`params_read`.
    pub params_bytes: Arc<Mutex<Option<Vec<u8>>>>,

    /// Active User-Agent string for HTTP and browser requests. Sidecars may read and update
    /// this via `ua_get_len`/`ua_get`/`ua_set`. Default: `brrmmmm/<version>`.
    pub user_agent: Arc<Mutex<String>>,

    /// Whether brrmmmm identity disclosure is added to remote requests. Sidecars may disable
    /// this for compatibility with endpoints that reject or over-index attestation metadata.
    pub identity_disclosure_visible: bool,

    /// Remote attestation identity for this brrmmmm installation. None only in explicit
    /// legacy/inspection modes.
    pub identity: Option<InstallationIdentity>,

    /// Per-sidecar mission state used for signed request envelopes and local behavior summary.
    pub mission: MissionState,
}

impl HostState {
    pub fn new(
        log_channel: bool,
        params_bytes: Arc<Mutex<Option<Vec<u8>>>>,
        module_hash: [u8; 32],
        identity: Option<InstallationIdentity>,
    ) -> Self {
        Self {
            artifact_store: Arc::new(Mutex::new(ArtifactStore::default())),
            pending_response: Arc::new(Mutex::new(None)),
            pending_browser_response: Arc::new(Mutex::new(None)),
            pending_ai_response: Arc::new(Mutex::new(None)),
            pending_kv_response: Arc::new(Mutex::new(None)),
            log_channel,
            params_bytes,
            user_agent: Arc::new(Mutex::new(format!("brrmmmm/{}", env!("CARGO_PKG_VERSION")))),
            identity_disclosure_visible: true,
            identity,
            mission: MissionState::new(module_hash),
        }
    }

    pub fn set_mission_describe(&mut self, describe: &SidecarDescribe) {
        self.mission.set_describe(describe);
    }

    pub fn record_activity(&mut self, cap: u8, event_tag: &str, normalized_event: &[u8]) {
        self.mission
            .record_activity(cap, event_tag, normalized_event);
    }

    pub fn signed_envelope_for_request(
        &mut self,
        cap: u8,
        event_tag: &str,
        normalized_event: &[u8],
        binding: &RequestBinding,
    ) -> Option<SignedEnvelope> {
        let request_count = self.mission.next_request(cap, event_tag, normalized_event);
        if !self.identity_disclosure_visible {
            return None;
        }
        let identity = self.identity.as_ref()?;
        let mut nonce = [0u8; 16];
        if let Err(error) = getrandom::fill(&mut nonce) {
            eprintln!("[brrmmmm] attestation nonce generation failed: {error}");
            return None;
        }
        let fields = EnvelopeFields {
            client_id: identity.client_id,
            mission_id: self.mission.mission_id,
            module_hash: self.mission.module_hash,
            request_count,
            behavior_hash: self.mission.behavior_hash,
            cap_mask: self.mission.cap_mask,
            timestamp_ms: now_ms(),
            nonce,
            key_id: identity.key_id,
            public_key: identity.public_key,
        };
        match attestation::build_signed_envelope(&fields, binding, &identity.signing_key) {
            Ok(envelope) => Some(envelope),
            Err(e) => {
                eprintln!("[brrmmmm] failed to build signed envelope: {e}");
                None
            }
        }
    }

    pub fn set_identity_disclosure_visible(&mut self, visible: bool) {
        self.identity_disclosure_visible = visible;
    }

    pub fn clear_transient_runtime_outputs(&mut self) {
        clear_mutex_option(&self.pending_response);
        clear_mutex_option(&self.pending_browser_response);
        clear_mutex_option(&self.pending_ai_response);
        clear_mutex_option(&self.pending_kv_response);
        lock_or_recover(&self.artifact_store, "artifact_store").clear();
    }

    pub fn full_user_agent(&self, envelope: Option<&SignedEnvelope>) -> String {
        let base = self
            .user_agent
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .trim()
            .to_string();
        if !self.identity_disclosure_visible {
            return base;
        }

        let marker = format!("brrmmmm/{}", env!("CARGO_PKG_VERSION"));
        let mut parts = Vec::new();
        if base.is_empty() {
            parts.push(marker);
        } else if base == marker {
            parts.push(base);
        } else {
            parts.push(base);
            parts.push(marker);
        }
        if let Some(envelope) = envelope {
            parts.push(envelope.user_agent_suffix.clone());
        }
        parts.join(" ")
    }
}

fn clear_mutex_option<T>(mutex: &Mutex<Option<T>>) {
    *lock_or_recover(mutex, "transient_response") = None;
}

fn lock_or_recover<'a, T>(mutex: &'a Mutex<T>, name: &str) -> std::sync::MutexGuard<'a, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            eprintln!("[brrmmmm] recovering poisoned {name} mutex");
            poisoned.into_inner()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use ed25519_dalek::SigningKey;

    use super::*;
    use crate::attestation::short_hash_16;

    fn identity() -> InstallationIdentity {
        let signing_key = SigningKey::from_bytes(&[7u8; 32]);
        let public_key = signing_key.verifying_key().to_bytes();
        InstallationIdentity {
            client_id: [1u8; 16],
            key_id: short_hash_16(b"brrmmmm-key-id-v1", &public_key),
            public_key,
            signing_key,
        }
    }

    #[test]
    fn visible_identity_appends_marker_and_readable_attestation() {
        let mut host = HostState::new(
            false,
            Arc::new(Mutex::new(None)),
            [3u8; 32],
            Some(identity()),
        );
        *host.user_agent.lock().unwrap() = "sidecar/1".to_string();
        let binding = RequestBinding::new("GET", "example.com", "/v1", None);
        let envelope = host
            .signed_envelope_for_request(0x01, "network", b"GET\nexample.com\n/v1", &binding)
            .unwrap();

        let ua = host.full_user_agent(Some(&envelope));

        assert!(ua.starts_with(&format!("sidecar/1 brrmmmm/{}", env!("CARGO_PKG_VERSION"))));
        assert!(ua.contains(" brrm/1 "));
        assert!(ua.contains(" cid/0101010101010101 "));
    }

    #[test]
    fn hidden_identity_uses_only_sidecar_user_agent() {
        let mut host = HostState::new(
            false,
            Arc::new(Mutex::new(None)),
            [3u8; 32],
            Some(identity()),
        );
        *host.user_agent.lock().unwrap() = "sidecar/2".to_string();
        host.set_identity_disclosure_visible(false);
        let binding = RequestBinding::new("GET", "example.com", "/v1", None);

        let envelope =
            host.signed_envelope_for_request(0x01, "network", b"GET\nexample.com\n/v1", &binding);

        assert!(envelope.is_none());
        assert_eq!(host.full_user_agent(None), "sidecar/2");
    }
}
