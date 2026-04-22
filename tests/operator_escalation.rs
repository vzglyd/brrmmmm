use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_brrmmmm"))
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn operator_action_fixture_wasm() -> PathBuf {
    static WASM: OnceLock<PathBuf> = OnceLock::new();
    WASM.get_or_init(|| {
        let root = repo_root();
        let manifest = root.join("fixtures/operator-action-module/Cargo.toml");
        let target_dir = root.join("target/test-fixtures/operator-action-module");
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
            .expect("failed to build operator-action-module fixture");
        assert!(
            status.success(),
            "operator-action-module fixture build failed"
        );
        target_dir.join("wasm32-wasip1/release/operator_action_module.wasm")
    })
    .clone()
}

fn temp_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "brrmmmm-operator-escalation-{label}-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn copy_fixture(dir: &Path, source: &Path, name: &str) -> PathBuf {
    let dest = dir.join(name);
    std::fs::copy(source, &dest).unwrap();
    dest
}

fn run_brr_in(dir: &Path, args: &[&str]) -> Output {
    Command::new(bin())
        .args(args)
        .current_dir(dir)
        .env("BRRMMMM_STATE_DIR", dir.join(".state"))
        .env("BRRMMMM_ATTESTATION", "off")
        .output()
        .expect("failed to run brrmmmm")
}

#[test]
fn inspect_reports_operator_fallback_contract() {
    let wasm = operator_action_fixture_wasm();
    let output = Command::new(bin())
        .args(["inspect", wasm.to_str().unwrap(), "--output", "json"])
        .env(
            "BRRMMMM_STATE_DIR",
            repo_root().join("target/test-state/operator-escalation-inspect"),
        )
        .env("BRRMMMM_ATTESTATION", "off")
        .output()
        .expect("failed to run brrmmmm inspect");

    assert!(
        output.status.success(),
        "inspect failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let inspection: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(inspection["abi_version"], 4);
    assert_eq!(inspection["describe"]["acquisition_timeout_secs"], 30);
    assert_eq!(
        inspection["describe"]["operator_fallback"]["timeout_ms"],
        60000
    );
    assert_eq!(
        inspection["describe"]["operator_fallback"]["on_timeout"],
        "terminal_failure"
    );
}

#[test]
fn explain_tracks_operator_rescue_window_before_and_after_expiry() {
    let dir = temp_dir("explain");
    let wasm = copy_fixture(&dir, &operator_action_fixture_wasm(), "mission-module.wasm");

    let run = run_brr_in(
        &dir,
        &[
            "run",
            wasm.file_name().unwrap().to_str().unwrap(),
            "--once",
            "--result-path",
            "mission.json",
        ],
    );
    assert_eq!(run.status.code(), Some(65));
    let record: Value =
        serde_json::from_slice(&std::fs::read(dir.join("mission.json")).unwrap()).unwrap();
    assert_eq!(record["schema_version"], 1);
    assert_eq!(record["outcome"]["status"], "operator_action_required");
    assert_eq!(record["escalation"]["timeout_outcome"], "retryable_failure");
    assert_eq!(record["host_decision"]["risk_posture"], "awaiting_operator");
    assert_eq!(
        record["host_decision"]["next_attempt_policy"],
        "operator_rescue"
    );

    let explain_open = run_brr_in(&dir, &["explain", "mission.json", "--output", "json"]);
    assert!(
        explain_open.status.success(),
        "explain failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&explain_open.stdout),
        String::from_utf8_lossy(&explain_open.stderr)
    );
    let open_view: Value = serde_json::from_slice(&explain_open.stdout).unwrap();
    assert_eq!(open_view["outcome"], "operator_action_required");
    assert_eq!(open_view["timeout_outcome"], "retryable_failure");
    assert_eq!(open_view["rescue_window_open"], true);
    assert_eq!(open_view["risk_posture"], "awaiting_operator");
    assert_eq!(open_view["next_attempt_policy"], "operator_rescue");

    std::thread::sleep(Duration::from_millis(300));

    let explain_expired = run_brr_in(&dir, &["explain", "mission.json", "--output", "json"]);
    assert!(
        explain_expired.status.success(),
        "explain failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&explain_expired.stdout),
        String::from_utf8_lossy(&explain_expired.stderr)
    );
    let expired_view: Value = serde_json::from_slice(&explain_expired.stdout).unwrap();
    assert_eq!(expired_view["outcome"], "retryable_failure");
    assert_eq!(expired_view["recorded_outcome"], "operator_action_required");
    assert_eq!(expired_view["category"], "retryable_failure");
    assert_eq!(expired_view["exit_code"], 75);
    assert_eq!(expired_view["rescue_window_open"], false);
    assert_eq!(expired_view["risk_posture"], "closed_safe");
    assert_eq!(expired_view["next_attempt_policy"], "after_cooldown");
    assert!(
        expired_view["basis"]
            .as_array()
            .is_some_and(|basis| { basis.iter().any(|value| value == "rescue_window_expired") })
    );
    assert!(
        expired_view["summary"]
            .as_str()
            .is_some_and(|summary| summary.contains("expired")),
        "summary:\n{}",
        expired_view["summary"]
    );
}

#[test]
fn rehearse_emits_scenarios_with_records_and_explanations() {
    let wasm = operator_action_fixture_wasm();
    let output = Command::new(bin())
        .args(["rehearse", wasm.to_str().unwrap(), "--output", "json"])
        .env(
            "BRRMMMM_STATE_DIR",
            repo_root().join("target/test-state/operator-escalation-rehearse"),
        )
        .env("BRRMMMM_ATTESTATION", "off")
        .output()
        .expect("failed to run brrmmmm rehearse");

    assert!(
        output.status.success(),
        "rehearse failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let scenarios: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(scenarios.as_array().is_some_and(|items| items.len() >= 4));
    assert!(scenarios.as_array().is_some_and(|items| {
        items.iter().any(|item| {
            item["scenario"] == "repeat_failure_gate"
                && item["explain"]["risk_posture"] == "awaiting_changed_conditions"
        })
    }));
    assert!(scenarios.as_array().is_some_and(|items| {
        items.iter().any(|item| {
            item["scenario"] == "operator_rescue_expired"
                && item["explain"]["risk_posture"] == "closed_safe"
        })
    }));
}
