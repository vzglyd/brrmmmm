//! Mission identity and behavior-summary tracking for request attestation.

use sha2::{Digest, Sha256};

use crate::abi::MissionModuleDescribe;
use crate::identity::{BehaviorHash, MissionId, ModuleHash};

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    /// Bitmask of host capabilities observed during the current mission.
    pub struct Capabilities: u8 {
        const NETWORK = 0x01;
        const BROWSER = 0x02;
        const AI = 0x04;
        const KV = 0x08;
    }
}

/// Tracks stable mission identity plus monotonic request and behavior summaries.
///
/// `MissionState` starts with an ID derived only from the module hash so the
/// runtime has a stable fallback before `describe()` is available. Once the
/// sidecar describe contract is known, [`Self::set_describe`] replaces that
/// fallback with a deterministic mission ID derived from the module hash and
/// describe metadata.
///
/// Invariants:
///
/// - `request_count` is monotonic and saturating.
/// - `capabilities` is the union of all capabilities observed so far.
/// - `behavior_hash` summarizes normalized events in call order.
#[derive(Debug, Clone)]
pub struct MissionState {
    module_hash: ModuleHash,
    mission_id: MissionId,
    request_count: u64,
    behavior_hash: BehaviorHash,
    capabilities: Capabilities,
}

impl MissionState {
    /// Construct mission state for one sidecar module.
    pub fn new(module_hash: ModuleHash) -> Self {
        Self {
            module_hash,
            mission_id: mission_id_without_describe(module_hash),
            request_count: 0,
            behavior_hash: BehaviorHash([0u8; 16]),
            capabilities: Capabilities::empty(),
        }
    }

    /// Replace the fallback mission ID with one derived from the describe contract.
    pub fn set_describe(&mut self, describe: &MissionModuleDescribe) {
        self.mission_id = mission_id_from_describe(self.module_hash, describe);
    }

    /// Record one normalized activity without incrementing the request counter.
    pub fn record_activity(
        &mut self,
        capability: Capabilities,
        event_tag: &str,
        normalized_event: &[u8],
    ) {
        self.capabilities.insert(capability);
        self.behavior_hash = next_behavior_hash(self.behavior_hash, event_tag, normalized_event);
    }

    /// Record one request and return the resulting 1-based request counter.
    ///
    /// The counter saturates at `u64::MAX` rather than wrapping.
    pub fn next_request(
        &mut self,
        capability: Capabilities,
        event_tag: &str,
        normalized_event: &[u8],
    ) -> u64 {
        self.record_activity(capability, event_tag, normalized_event);
        self.request_count = self.request_count.saturating_add(1);
        self.request_count
    }

    /// Return the stable hash of the loaded sidecar module bytes.
    pub fn module_hash(&self) -> ModuleHash {
        self.module_hash
    }

    /// Return the current mission identifier.
    pub fn mission_id(&self) -> MissionId {
        self.mission_id
    }

    /// Return the cumulative behavior hash for normalized activity so far.
    pub fn behavior_hash(&self) -> BehaviorHash {
        self.behavior_hash
    }

    /// Return the observed capability bitmask.
    pub fn cap_mask(&self) -> u8 {
        self.capabilities.bits()
    }
}

/// Derive a fallback mission ID from the module hash alone.
pub fn mission_id_without_describe(module_hash: ModuleHash) -> MissionId {
    let mut hasher = Sha256::new();
    push_str(&mut hasher, "brrmmmm-mission-v1");
    push_bytes(&mut hasher, module_hash.as_bytes());
    short(hasher.finalize())
}

/// Derive a deterministic mission ID from the module hash and sidecar describe metadata.
///
/// The function clones and sorts `capabilities_needed` and `run_modes` before
/// hashing so the result is insensitive to ordering differences in the describe
/// payload. Complexity is `O(c log c + m log m)`, where `c` is the number of
/// declared capabilities and `m` is the number of declared run modes.
pub fn mission_id_from_describe(
    module_hash: ModuleHash,
    describe: &MissionModuleDescribe,
) -> MissionId {
    let mut capabilities = describe.capabilities_needed.clone();
    capabilities.sort();
    let mut modes = describe.run_modes.clone();
    modes.sort();

    let mut hasher = Sha256::new();
    push_str(&mut hasher, "brrmmmm-mission-v1");
    push_bytes(&mut hasher, module_hash.as_bytes());
    push_str(&mut hasher, &describe.logical_id);
    hasher.update(describe.abi_version.to_be_bytes());
    for capability in capabilities {
        push_str(&mut hasher, &capability);
    }
    for mode in modes {
        push_str(&mut hasher, &mode);
    }
    short(hasher.finalize())
}

/// Normalize an HTTP request into the behavior-hash event payload format.
pub fn network_event(method: &str, authority: &str, path: &str) -> Vec<u8> {
    format!(
        "{}\n{}\n{}",
        method.to_ascii_uppercase(),
        authority.to_ascii_lowercase(),
        normalize_path_for_behavior(path)
    )
    .into_bytes()
}

/// Normalize a browser action kind into the behavior-hash event payload format.
pub fn browser_action_event(action_kind: &str) -> Vec<u8> {
    format!("action={action_kind}").into_bytes()
}

/// Normalize an AI action kind into the behavior-hash event payload format.
pub fn ai_event(action_kind: &str) -> Vec<u8> {
    format!("provider=anthropic\noperation={action_kind}").into_bytes()
}

/// Normalize a KV operation kind into the behavior-hash event payload format.
pub fn kv_event(operation_kind: &str) -> Vec<u8> {
    format!("operation={operation_kind}").into_bytes()
}

fn next_behavior_hash(
    previous: BehaviorHash,
    event_tag: &str,
    normalized_event: &[u8],
) -> BehaviorHash {
    let mut hasher = Sha256::new();
    hasher.update(previous.as_bytes());
    push_str(&mut hasher, event_tag);
    push_bytes(&mut hasher, normalized_event);
    BehaviorHash(short_bytes(hasher.finalize()))
}

fn normalize_path_for_behavior(path: &str) -> String {
    let path = path.split('?').next().unwrap_or(path);
    if path.is_empty() {
        return "/".to_string();
    }
    let mut normalized = String::new();
    for segment in path.split('/') {
        if segment.is_empty() {
            if normalized.is_empty() {
                normalized.push('/');
            }
            continue;
        }
        if !normalized.ends_with('/') {
            normalized.push('/');
        }
        if looks_sensitive_identifier(segment) {
            normalized.push_str(":id");
        } else {
            normalized.push_str(segment);
        }
    }
    if normalized.is_empty() {
        "/".to_string()
    } else {
        normalized
    }
}

fn looks_sensitive_identifier(segment: &str) -> bool {
    let long_hex = segment.len() >= 8 && segment.chars().all(|ch| ch.is_ascii_hexdigit());
    let long_token = segment.len() >= 24 && segment.chars().all(|ch| ch.is_ascii_alphanumeric());
    let all_digits = segment.chars().all(|ch| ch.is_ascii_digit());
    all_digits || long_hex || long_token
}

fn push_str(hasher: &mut Sha256, value: &str) {
    push_bytes(hasher, value.as_bytes());
}

fn push_bytes(hasher: &mut Sha256, value: &[u8]) {
    let len = u32::try_from(value.len()).unwrap_or(u32::MAX);
    hasher.update(len.to_be_bytes());
    hasher.update(&value[..len as usize]);
}

fn short(digest: impl AsRef<[u8]>) -> MissionId {
    MissionId(short_bytes(digest))
}

fn short_bytes(digest: impl AsRef<[u8]>) -> [u8; 16] {
    let mut out = [0u8; 16];
    out.copy_from_slice(&digest.as_ref()[..16]);
    out
}

#[cfg(test)]
mod tests {
    use crate::abi::PersistenceAuthority;

    use super::*;

    fn describe(capabilities: Vec<String>) -> MissionModuleDescribe {
        MissionModuleDescribe {
            schema_version: 1,
            logical_id: "brrmmmm.test".to_string(),
            name: "Test".to_string(),
            description: "Test sidecar".to_string(),
            abi_version: 1,
            run_modes: vec!["managed_polling".to_string()],
            state_persistence: PersistenceAuthority::Volatile,
            required_env_vars: vec![],
            optional_env_vars: vec![],
            params: None,
            capabilities_needed: capabilities,
            poll_strategy: None,
            cooldown_policy: None,
            artifact_types: vec!["published_output".to_string()],
            acquisition_timeout_secs: None,
            operator_fallback: None,
        }
    }

    #[test]
    fn mission_id_changes_when_capabilities_change() {
        let module_hash = ModuleHash([9u8; 32]);
        let a = mission_id_from_describe(module_hash, &describe(vec!["network".to_string()]));
        let b = mission_id_from_describe(module_hash, &describe(vec!["browser".to_string()]));

        assert_ne!(a, b);
    }

    #[test]
    fn request_count_increments_and_cap_mask_accumulates() {
        let mut state = MissionState::new(ModuleHash([1u8; 32]));
        let first = state.next_request(Capabilities::NETWORK, "network", b"GET\nexample.com\n/");
        let second = state.next_request(Capabilities::AI, "ai", b"complete");

        assert_eq!(first, 1);
        assert_eq!(second, 2);
        assert_eq!(
            state.cap_mask(),
            (Capabilities::NETWORK | Capabilities::AI).bits()
        );
        assert_ne!(state.behavior_hash(), BehaviorHash([0u8; 16]));
    }

    #[test]
    fn behavior_path_normalization_removes_common_identifiers() {
        assert_eq!(
            normalize_path_for_behavior("/users/12345/items/abcdef12?token=secret"),
            "/users/:id/items/:id"
        );
    }
}
