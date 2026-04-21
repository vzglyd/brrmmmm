use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

use serde_json::Value;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_brrmmmm"))
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn fixture_wasm() -> PathBuf {
    static WASM: OnceLock<PathBuf> = OnceLock::new();
    WASM.get_or_init(|| {
        let root = repo_root();
        let manifest = root.join("fixtures/deterministic-sidecar/Cargo.toml");
        let target_dir = root.join("target/test-fixtures/deterministic-sidecar");
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
            .expect("failed to build deterministic sidecar fixture");
        assert!(
            status.success(),
            "deterministic sidecar fixture build failed"
        );
        target_dir.join("wasm32-wasip1/release/deterministic_sidecar.wasm")
    })
    .clone()
}

fn run_brr(args: &[&str]) -> std::process::Output {
    Command::new(bin())
        .args(args)
        .env(
            "BRRMMMM_STATE_DIR",
            repo_root().join("target/test-state/cli-commands"),
        )
        .env("BRRMMMM_ATTESTATION", "off")
        .output()
        .expect("failed to run brrmmmm")
}

#[test]
fn validate_accepts_deterministic_fixture() {
    let wasm = fixture_wasm();
    let output = run_brr(&["validate", wasm.to_str().unwrap()]);

    assert!(
        output.status.success(),
        "validate failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("WASM module validates successfully"));
    assert!(stderr.contains("Deterministic Fixture Mission Module"));
    assert!(stderr.contains("managed_polling, interactive"));
}

#[test]
fn inspect_prints_real_contract_json() {
    let wasm = fixture_wasm();
    let output = run_brr(&["inspect", wasm.to_str().unwrap()]);

    assert!(
        output.status.success(),
        "inspect failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let json: Value = serde_json::from_slice(&output.stdout).expect("inspect stdout is JSON");
    assert_eq!(json["abi_version"], 4);
    assert_eq!(
        json["describe"]["logical_id"],
        "brrmmmm.fixture.deterministic"
    );
    assert_eq!(json["describe"]["artifact_types"][2], "published_output");
    assert_eq!(json["entrypoint"], "brrmmmm_module_start");
    assert_eq!(json["assurance_defaults"]["same_reason_retry_limit"], 3);
    assert_eq!(
        json["assurance_defaults"]["default_retry_after_ms"],
        300_000
    );
    assert!(json["host_imports"].as_array().is_some_and(|imports| {
        imports
            .iter()
            .any(|value| value == "mission_outcome_report")
    }));
}

#[test]
fn run_once_prints_only_published_payload() {
    let wasm = fixture_wasm();
    let output = run_brr(&["run", wasm.to_str().unwrap(), "--once"]);

    assert!(
        output.status.success(),
        "run --once failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let payload: Value = serde_json::from_slice(&output.stdout).expect("payload stdout is JSON");
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["source"], "fixture");
    assert_eq!(payload["count"], 3);
}

#[test]
fn events_mode_outputs_ndjson_without_payload_leakage() {
    let wasm = fixture_wasm();
    let output = run_brr(&["run", wasm.to_str().unwrap(), "--once", "--events"]);

    assert!(
        output.status.success(),
        "run --once --events failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let events = stdout
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("event line is JSON"))
        .collect::<Vec<_>>();

    assert!(has_event(&events, "env_snapshot"));
    assert!(has_event(&events, "started"));
    assert!(has_event(&events, "describe"));
    assert!(events.iter().any(|event| {
        event["type"] == "mission_outcome"
            && event["host_decision"]["risk_posture"] == "nominal"
            && event["host_decision"]["next_attempt_policy"] == "none"
    }));
    assert!(events.iter().any(|event| {
        event["type"] == "artifact_received" && event["kind"] == "published_output"
    }));
    assert!(!events.iter().any(|event| event["ok"] == true));
}

#[test]
fn invalid_config_exits_with_input_code() {
    let wasm = fixture_wasm();
    let output = Command::new(bin())
        .args(["--log-format", "json", "validate", wasm.to_str().unwrap()])
        .env(
            "BRRMMMM_STATE_DIR",
            repo_root().join("target/test-state/cli-invalid-config"),
        )
        .env("BRRMMMM_BROWSER_HEADLESS", "maybe")
        .env("BRRMMMM_ATTESTATION", "off")
        .output()
        .expect("failed to run brrmmmm");

    assert_eq!(output.status.code(), Some(64));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    let event: Value = serde_json::from_str(stderr.trim()).expect("stderr is JSON");
    assert_eq!(event["level"], "error");
    assert_eq!(event["category"], "config_invalid");
    assert!(
        event["message"]
            .as_str()
            .is_some_and(|message| message.contains("invalid configuration")),
        "stderr:\n{stderr}"
    );
}

fn has_event(events: &[Value], kind: &str) -> bool {
    events.iter().any(|event| event["type"] == kind)
}
