use sha2::{Digest, Sha256};

use crate::abi::SidecarDescribe;

pub const CAP_NETWORK: u8 = 0x01;
pub const CAP_BROWSER: u8 = 0x02;
pub const CAP_AI: u8 = 0x04;
pub const CAP_KV: u8 = 0x08;

#[derive(Debug, Clone)]
pub struct MissionState {
    pub module_hash: [u8; 32],
    pub mission_id: [u8; 16],
    pub request_count: u64,
    pub behavior_hash: [u8; 16],
    pub cap_mask: u8,
}

impl MissionState {
    pub fn new(module_hash: [u8; 32]) -> Self {
        Self {
            module_hash,
            mission_id: mission_id_without_describe(module_hash),
            request_count: 0,
            behavior_hash: [0u8; 16],
            cap_mask: 0,
        }
    }

    pub fn set_describe(&mut self, describe: &SidecarDescribe) {
        self.mission_id = mission_id_from_describe(self.module_hash, describe);
    }

    pub fn record_activity(&mut self, cap: u8, event_tag: &str, normalized_event: &[u8]) {
        self.cap_mask |= cap;
        self.behavior_hash = next_behavior_hash(self.behavior_hash, event_tag, normalized_event);
    }

    pub fn next_request(&mut self, cap: u8, event_tag: &str, normalized_event: &[u8]) -> u64 {
        self.record_activity(cap, event_tag, normalized_event);
        self.request_count = self.request_count.saturating_add(1);
        self.request_count
    }
}

pub fn mission_id_without_describe(module_hash: [u8; 32]) -> [u8; 16] {
    let mut hasher = Sha256::new();
    push_str(&mut hasher, "brrmmmm-mission-v1");
    push_bytes(&mut hasher, &module_hash);
    short(hasher.finalize())
}

pub fn mission_id_from_describe(module_hash: [u8; 32], describe: &SidecarDescribe) -> [u8; 16] {
    let mut capabilities = describe.capabilities_needed.clone();
    capabilities.sort();
    let mut modes = describe.run_modes.clone();
    modes.sort();

    let mut hasher = Sha256::new();
    push_str(&mut hasher, "brrmmmm-mission-v1");
    push_bytes(&mut hasher, &module_hash);
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

pub fn network_event(method: &str, authority: &str, path: &str) -> Vec<u8> {
    format!(
        "{}\n{}\n{}",
        method.to_ascii_uppercase(),
        authority.to_ascii_lowercase(),
        normalize_path_for_behavior(path)
    )
    .into_bytes()
}

pub fn browser_action_event(action_kind: &str) -> Vec<u8> {
    format!("action={action_kind}").into_bytes()
}

pub fn ai_event(action_kind: &str) -> Vec<u8> {
    format!("provider=anthropic\noperation={action_kind}").into_bytes()
}

pub fn kv_event(operation_kind: &str) -> Vec<u8> {
    format!("operation={operation_kind}").into_bytes()
}

fn next_behavior_hash(previous: [u8; 16], event_tag: &str, normalized_event: &[u8]) -> [u8; 16] {
    let mut hasher = Sha256::new();
    hasher.update(previous);
    push_str(&mut hasher, event_tag);
    push_bytes(&mut hasher, normalized_event);
    short(hasher.finalize())
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

fn short(digest: impl AsRef<[u8]>) -> [u8; 16] {
    let mut out = [0u8; 16];
    out.copy_from_slice(&digest.as_ref()[..16]);
    out
}

#[cfg(test)]
mod tests {
    use crate::abi::PersistenceAuthority;

    use super::*;

    fn describe(capabilities: Vec<String>) -> SidecarDescribe {
        SidecarDescribe {
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
        }
    }

    #[test]
    fn mission_id_changes_when_capabilities_change() {
        let module_hash = [9u8; 32];
        let a = mission_id_from_describe(module_hash, &describe(vec!["network".to_string()]));
        let b = mission_id_from_describe(module_hash, &describe(vec!["browser".to_string()]));

        assert_ne!(a, b);
    }

    #[test]
    fn request_count_increments_and_cap_mask_accumulates() {
        let mut state = MissionState::new([1u8; 32]);
        let first = state.next_request(CAP_NETWORK, "network", b"GET\nexample.com\n/");
        let second = state.next_request(CAP_AI, "ai", b"complete");

        assert_eq!(first, 1);
        assert_eq!(second, 2);
        assert_eq!(state.cap_mask, CAP_NETWORK | CAP_AI);
        assert_ne!(state.behavior_hash, [0u8; 16]);
    }

    #[test]
    fn behavior_path_normalization_removes_common_identifiers() {
        assert_eq!(
            normalize_path_for_behavior("/users/12345/items/abcdef12?token=secret"),
            "/users/:id/items/:id"
        );
    }
}
