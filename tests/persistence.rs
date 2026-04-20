use std::time::{SystemTime, UNIX_EPOCH};

use brrmmmm::abi::SidecarRuntimeState;
use brrmmmm::config::Config;
use brrmmmm::error::BrrmmmmError;
use brrmmmm::persistence::wasm_identity;

fn temp_state_dir() -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("brrmmmm-persistence-test-{nanos}"))
}

fn config_with_temp_state() -> Config {
    let mut config = Config::load().expect("test config");
    config.state_dir = temp_state_dir();
    config
}

#[test]
fn wasm_identity_empty_slice_is_fnv1a_offset_basis() {
    // FNV-1a 64-bit: no bytes XOR'd, so result is the offset basis in hex.
    assert_eq!(wasm_identity(&[]), "cbf29ce484222325");
}

#[test]
fn wasm_identity_is_deterministic() {
    let data = b"brrmmmm sidecar runtime";
    assert_eq!(wasm_identity(data), wasm_identity(data));
}

#[test]
fn wasm_identity_output_is_16_lowercase_hex_chars() {
    let result = wasm_identity(b"hello");
    assert_eq!(result.len(), 16);
    assert!(
        result.chars().all(|c| c.is_ascii_hexdigit()),
        "non-hex chars in: {result}"
    );
    assert!(
        result.chars().all(|c| !c.is_ascii_uppercase()),
        "uppercase chars in: {result}"
    );
}

#[test]
fn wasm_identity_different_inputs_produce_different_hashes() {
    assert_ne!(wasm_identity(b"alpha"), wasm_identity(b"beta"));
}

#[test]
fn wasm_identity_single_byte_change_produces_different_hash() {
    let a = b"hello";
    let b = b"hellp";
    assert_ne!(wasm_identity(a), wasm_identity(b));
}

#[test]
fn load_returns_none_for_missing_state_file() {
    let config = config_with_temp_state();
    let result = brrmmmm::persistence::load(&config, "missing").unwrap();
    assert!(result.is_none());
}

#[test]
fn load_returns_error_for_corrupted_state_file() {
    let config = config_with_temp_state();
    std::fs::create_dir_all(&config.state_dir).unwrap();
    std::fs::write(config.state_dir.join("bad.json"), b"not json").unwrap();

    let result = brrmmmm::persistence::load(&config, "bad");

    assert!(matches!(result, Err(BrrmmmmError::StateCorruption(_))));
    let _ = std::fs::remove_dir_all(config.state_dir);
}

#[test]
fn save_then_load_roundtrips_state_atomically() {
    let config = config_with_temp_state();
    let mut state = SidecarRuntimeState::default();
    state.kv.insert("token".to_string(), b"secret".to_vec());

    brrmmmm::persistence::save(&config, "roundtrip", &state).unwrap();
    let loaded = brrmmmm::persistence::load(&config, "roundtrip")
        .unwrap()
        .expect("state exists");

    assert_eq!(loaded.kv.get("token"), Some(&b"secret".to_vec()));
    let _ = std::fs::remove_dir_all(config.state_dir);
}

#[test]
fn save_rejects_state_over_kv_quota() {
    let mut config = config_with_temp_state();
    config.limits.kv_max_total_bytes = 16;
    let mut state = SidecarRuntimeState::default();
    state
        .kv
        .insert("large".to_string(), b"more-than-sixteen-bytes".to_vec());

    let result = brrmmmm::persistence::save(&config, "quota", &state);

    assert!(matches!(result, Err(BrrmmmmError::BudgetExceeded { .. })));
    assert!(!config.state_dir.join("quota.json").exists());
    let _ = std::fs::remove_dir_all(config.state_dir);
}
