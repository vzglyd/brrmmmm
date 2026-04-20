use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use brrmmmm::abi::SidecarRuntimeState;
use brrmmmm::persistence;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_brrmmmm"))
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn kv_fixture_wasm() -> PathBuf {
    static WASM: OnceLock<PathBuf> = OnceLock::new();
    WASM.get_or_init(|| {
        let root = repo_root();
        let manifest = root.join("fixtures/kv-sidecar/Cargo.toml");
        let target_dir = root.join("target/test-fixtures/kv-sidecar");
        let status = Command::new("cargo")
            .args([
                "build",
                "--manifest-path",
                manifest.to_str().unwrap(),
                "--target",
                "wasm32-wasip1",
                "--release",
            ])
            .env("CARGO_TARGET_DIR", &target_dir)
            .status()
            .expect("failed to build kv sidecar fixture");
        assert!(status.success(), "kv sidecar fixture build failed");
        target_dir.join("wasm32-wasip1/release/kv_sidecar.wasm")
    })
    .clone()
}

fn temp_state_dir() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("brrmmmm-kv-test-{}-{nanos}", std::process::id()))
}

#[test]
fn kv_storage_roundtrips_in_runtime_state() {
    let mut state = SidecarRuntimeState::default();
    let key = "session_id".to_string();
    let value = b"abc-123".to_vec();

    state.kv.insert(key.clone(), value.clone());
    assert_eq!(state.kv.get(&key), Some(&value));

    state.kv.remove(&key);
    assert!(state.kv.get(&key).is_none());
}

#[test]
fn kv_sidecar_uses_imports_and_persists_host_state() {
    let wasm = kv_fixture_wasm();
    let state_dir = temp_state_dir();
    let output = Command::new(bin())
        .args(["run", wasm.to_str().unwrap(), "--once", "--output", "json"])
        .env("BRRMMMM_STATE_DIR", &state_dir)
        .env("BRRMMMM_ATTESTATION", "off")
        .output()
        .expect("failed to run brrmmmm");

    assert!(
        output.status.success(),
        "kv sidecar run failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let payload: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("run stdout is JSON");
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["roundtrip"], "abc-123");
    assert_eq!(payload["deleted_missing"], true);

    let wasm_bytes = std::fs::read(&wasm).unwrap();
    let wasm_hash = persistence::wasm_identity(&wasm_bytes);
    let state_path = state_dir.join(format!("{wasm_hash}.json"));
    let state_bytes = std::fs::read(&state_path).unwrap_or_else(|error| {
        panic!(
            "failed to read persisted state at {}: {error}",
            state_path.display()
        )
    });
    let state: SidecarRuntimeState =
        serde_json::from_slice(&state_bytes).expect("persisted state is JSON");

    assert_eq!(
        state.kv.get("persisted_token"),
        Some(&b"secret-token".to_vec())
    );
    assert!(!state.kv.contains_key("session_id"));

    let _ = std::fs::remove_dir_all(state_dir);
}

#[test]
fn kv_sidecar_reports_error_when_state_cannot_be_persisted() {
    let wasm = kv_fixture_wasm();
    let state_path = temp_state_dir();
    std::fs::write(&state_path, b"not a directory").unwrap();

    let output = Command::new(bin())
        .args(["run", wasm.to_str().unwrap(), "--once", "--output", "json"])
        .env("BRRMMMM_STATE_DIR", &state_path)
        .env("BRRMMMM_ATTESTATION", "off")
        .output()
        .expect("failed to run brrmmmm");

    assert!(
        output.status.success(),
        "kv sidecar run failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let payload: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("run stdout is JSON");
    assert_eq!(payload["ok"], false);
    assert_eq!(payload["error"], "kv_set failed");

    let _ = std::fs::remove_file(state_path);
}
