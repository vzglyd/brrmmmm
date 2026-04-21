use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_brrmmmm"))
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn build_fixture(
    manifest_rel: &str,
    target_rel: &str,
    output_rel: &str,
    cache: &'static OnceLock<PathBuf>,
) -> PathBuf {
    cache
        .get_or_init(|| {
            let root = repo_root();
            let manifest = root.join(manifest_rel);
            let target_dir = root.join(target_rel);
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
                .expect("failed to build fixture");
            assert!(
                status.success(),
                "fixture build failed: {}",
                manifest.display()
            );
            target_dir.join(output_rel)
        })
        .clone()
}

fn deterministic_fixture_wasm() -> PathBuf {
    static WASM: OnceLock<PathBuf> = OnceLock::new();
    build_fixture(
        "fixtures/deterministic-sidecar/Cargo.toml",
        "target/test-fixtures/deterministic-sidecar",
        "wasm32-wasip1/release/deterministic_sidecar.wasm",
        &WASM,
    )
}

fn env_fixture_wasm() -> PathBuf {
    static WASM: OnceLock<PathBuf> = OnceLock::new();
    build_fixture(
        "fixtures/env-sidecar/Cargo.toml",
        "target/test-fixtures/env-sidecar",
        "wasm32-wasip1/release/env_sidecar.wasm",
        &WASM,
    )
}

fn params_fixture_wasm() -> PathBuf {
    static WASM: OnceLock<PathBuf> = OnceLock::new();
    build_fixture(
        "fixtures/params-sidecar/Cargo.toml",
        "target/test-fixtures/params-sidecar",
        "wasm32-wasip1/release/params_sidecar.wasm",
        &WASM,
    )
}

fn raw_output_fixture_wasm() -> PathBuf {
    static WASM: OnceLock<PathBuf> = OnceLock::new();
    build_fixture(
        "fixtures/raw-output-sidecar/Cargo.toml",
        "target/test-fixtures/raw-output-sidecar",
        "wasm32-wasip1/release/raw_output_sidecar.wasm",
        &WASM,
    )
}

fn timeout_fixture_wasm() -> PathBuf {
    static WASM: OnceLock<PathBuf> = OnceLock::new();
    build_fixture(
        "fixtures/timeout-sidecar/Cargo.toml",
        "target/test-fixtures/timeout-sidecar",
        "wasm32-wasip1/release/timeout_sidecar.wasm",
        &WASM,
    )
}

fn temp_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "brrmmmm-working-dir-{label}-{}-{nanos}",
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

fn write_config(dir: &Path, contents: &str) {
    std::fs::write(dir.join("brrmmmm.toml"), contents).unwrap();
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

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[test]
fn bare_brrmmmm_runs_cwd_config_and_writes_result_file() {
    let dir = temp_dir("bare-run");
    let wasm_path = copy_fixture(&dir, &deterministic_fixture_wasm(), "sidecar.wasm");
    std::fs::write(dir.join("mission.json"), "old-data").unwrap();
    write_config(
        &dir,
        r#"[mission]
wasm = "sidecar.wasm"
result_path = "mission.json"
"#,
    );

    let output = run_brr_in(&dir, &[]);

    assert!(
        output.status.success(),
        "bare run failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stdout.is_empty(),
        "stdout must stay quiet when result_path is configured"
    );

    let record = read_json(&dir.join("mission.json"));
    assert_eq!(record["outcome"]["status"], "published");
    assert_eq!(
        record["module"]["wasm_path"],
        wasm_path.display().to_string()
    );
    assert_eq!(record["artifacts"]["published_output"]["json"]["ok"], true);
    assert!(
        record["artifacts"]["published_output"]["text"]
            .as_str()
            .is_some()
    );
}

#[test]
fn run_without_positional_wasm_uses_config_file() {
    let dir = temp_dir("run-default-wasm");
    copy_fixture(&dir, &deterministic_fixture_wasm(), "sidecar.wasm");
    write_config(
        &dir,
        r#"[mission]
wasm = "sidecar.wasm"
"#,
    );

    let output = run_brr_in(&dir, &["run", "--once", "--output", "json"]);

    assert!(
        output.status.success(),
        "run failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let payload: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["count"], 3);
}

#[test]
fn inspect_and_validate_use_configured_wasm_path() {
    let dir = temp_dir("inspect-validate-default-wasm");
    copy_fixture(&dir, &deterministic_fixture_wasm(), "sidecar.wasm");
    write_config(
        &dir,
        r#"[mission]
wasm = "sidecar.wasm"
"#,
    );

    let inspect = run_brr_in(&dir, &["inspect"]);
    assert!(
        inspect.status.success(),
        "inspect failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&inspect.stdout),
        String::from_utf8_lossy(&inspect.stderr)
    );
    let inspection: Value = serde_json::from_slice(&inspect.stdout).unwrap();
    assert_eq!(
        inspection["describe"]["logical_id"],
        "brrmmmm.fixture.deterministic"
    );

    let validate = run_brr_in(&dir, &["validate", "--output", "json"]);
    assert!(
        validate.status.success(),
        "validate failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&validate.stdout),
        String::from_utf8_lossy(&validate.stderr)
    );
    let validation: Value = serde_json::from_slice(&validate.stdout).unwrap();
    assert_eq!(validation["valid"], true);
    assert_eq!(validation["logical_id"], "brrmmmm.fixture.deterministic");
}

#[test]
fn cli_params_override_config_params_file() {
    let dir = temp_dir("params-override");
    copy_fixture(&dir, &params_fixture_wasm(), "sidecar.wasm");
    write_config(
        &dir,
        r#"[mission]
wasm = "sidecar.wasm"
params_file = "missing.json"
"#,
    );

    let output = run_brr_in(
        &dir,
        &["run", "--once", "--params-json", r#"{"source":"cli"}"#],
    );

    assert!(
        output.status.success(),
        "run failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let payload: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(payload["params"]["source"], "cli");
}

#[test]
fn cli_result_path_overrides_config_result_path() {
    let dir = temp_dir("result-path-override");
    copy_fixture(&dir, &deterministic_fixture_wasm(), "sidecar.wasm");
    write_config(
        &dir,
        r#"[mission]
wasm = "sidecar.wasm"
result_path = "configured.json"
"#,
    );

    let output = run_brr_in(&dir, &["run", "--once", "--result-path", "cli.json"]);

    assert!(
        output.status.success(),
        "run failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    assert!(dir.join("cli.json").exists());
    assert!(!dir.join("configured.json").exists());
}

#[test]
fn config_env_and_cli_env_are_merged_with_cli_precedence() {
    let dir = temp_dir("env-merge");
    copy_fixture(&dir, &env_fixture_wasm(), "sidecar.wasm");
    write_config(
        &dir,
        r#"[mission]
wasm = "sidecar.wasm"

[mission.env]
FIXTURE_LABEL = "from-config"
EXTRA_LABEL = "from-config"
"#,
    );

    let output = run_brr_in(
        &dir,
        &[
            "run",
            "--once",
            "--env",
            "FIXTURE_LABEL=from-cli",
            "--output",
            "json",
        ],
    );

    assert!(
        output.status.success(),
        "run failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let payload: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(payload["label"], "from-cli");
    assert_eq!(payload["extra"], "from-config");
}

#[test]
fn events_mode_can_write_result_file_and_stdout_events() {
    let dir = temp_dir("events-plus-file");
    copy_fixture(&dir, &deterministic_fixture_wasm(), "sidecar.wasm");

    let output = run_brr_in(
        &dir,
        &[
            "run",
            "sidecar.wasm",
            "--once",
            "--events",
            "--result-path",
            "mission.json",
        ],
    );

    assert!(
        output.status.success(),
        "run failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let events = stdout
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert!(events.iter().any(|event| event["type"] == "started"));
    assert!(events.iter().any(|event| {
        event["type"] == "artifact_received" && event["kind"] == "published_output"
    }));

    let record = read_json(&dir.join("mission.json"));
    assert_eq!(record["outcome"]["status"], "published");
    assert_eq!(record["artifacts"]["published_output"]["json"]["ok"], true);
}

#[test]
fn timeout_writes_failure_mission_record() {
    let dir = temp_dir("timeout-record");
    copy_fixture(&dir, &timeout_fixture_wasm(), "sidecar.wasm");

    let output = run_brr_in(
        &dir,
        &[
            "run",
            "sidecar.wasm",
            "--once",
            "--result-path",
            "mission.json",
        ],
    );

    assert_eq!(output.status.code(), Some(124));
    assert!(output.stdout.is_empty());

    let record = read_json(&dir.join("mission.json"));
    assert_eq!(record["outcome"]["status"], "retryable_failure");
    assert_eq!(record["outcome"]["reason_code"], "acquisition_timeout");
    assert_eq!(record["host_decision"]["category"], "timeout");
    assert_eq!(record["host_decision"]["exit_code"], 124);
    assert_eq!(record["host_decision"]["risk_posture"], "degraded");
    assert_eq!(record["host_decision"]["next_attempt_policy"], "after_cooldown");
    assert!(record["artifacts"]["published_output"].is_null());
}

#[test]
fn repeat_failure_gate_requires_changed_conditions_until_overridden() {
    let dir = temp_dir("repeat-failure-gate");
    copy_fixture(&dir, &timeout_fixture_wasm(), "sidecar.wasm");
    write_config(
        &dir,
        r#"[mission]
wasm = "sidecar.wasm"

[assurance]
same_reason_retry_limit = 2
default_retry_after_ms = 50
"#,
    );

    let first = run_brr_in(
        &dir,
        &["run", "--once", "--result-path", "mission.json"],
    );
    assert_eq!(first.status.code(), Some(124));
    let first_record = read_json(&dir.join("mission.json"));
    assert_eq!(first_record["outcome"]["reason_code"], "acquisition_timeout");

    let second = run_brr_in(
        &dir,
        &["run", "--once", "--result-path", "mission.json"],
    );
    assert_eq!(second.status.code(), Some(124));

    let gated = run_brr_in(
        &dir,
        &["run", "--once", "--result-path", "mission.json"],
    );
    assert_eq!(gated.status.code(), Some(75));
    let gated_record = read_json(&dir.join("mission.json"));
    assert_eq!(gated_record["outcome"]["reason_code"], "changed_conditions_required");
    assert_eq!(
        gated_record["host_decision"]["risk_posture"],
        "awaiting_changed_conditions"
    );
    assert_eq!(
        gated_record["host_decision"]["next_attempt_policy"],
        "manual_only"
    );
    assert_eq!(gated_record["host_decision"]["synthesized"], true);
    let explain = run_brr_in(&dir, &["explain", "mission.json", "--output", "json"]);
    assert!(
        explain.status.success(),
        "explain failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&explain.stdout),
        String::from_utf8_lossy(&explain.stderr)
    );
    let explain_view: Value = serde_json::from_slice(&explain.stdout).unwrap();
    assert!(explain_view["next_action"]
        .as_str()
        .is_some_and(|value| value.contains("Change the inputs")));

    let override_attempt = run_brr_in(
        &dir,
        &[
            "run",
            "--once",
            "--override-retry-gate",
            "--result-path",
            "mission.json",
        ],
    );
    assert_eq!(override_attempt.status.code(), Some(124));
    let override_record = read_json(&dir.join("mission.json"));
    assert_eq!(override_record["outcome"]["reason_code"], "acquisition_timeout");
}

#[test]
fn raw_output_record_uses_base64_without_json_or_text() {
    let dir = temp_dir("raw-output-record");
    copy_fixture(&dir, &raw_output_fixture_wasm(), "sidecar.wasm");

    let output = run_brr_in(
        &dir,
        &[
            "run",
            "sidecar.wasm",
            "--once",
            "--result-path",
            "mission.json",
        ],
    );

    assert!(
        output.status.success(),
        "run failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());

    let record = read_json(&dir.join("mission.json"));
    assert_eq!(record["outcome"]["status"], "published");
    assert_eq!(record["artifacts"]["published_output"]["size_bytes"], 4);
    assert_eq!(
        record["artifacts"]["published_output"]["base64"],
        "/wB/gA=="
    );
    assert!(
        record["artifacts"]["published_output"]
            .get("json")
            .is_none()
    );
    assert!(
        record["artifacts"]["published_output"]
            .get("text")
            .is_none()
    );
}

#[test]
fn conflicting_config_params_sources_fail_with_input_code() {
    let dir = temp_dir("conflicting-config-params");
    copy_fixture(&dir, &deterministic_fixture_wasm(), "sidecar.wasm");
    write_config(
        &dir,
        r#"[mission]
wasm = "sidecar.wasm"
params_file = "params.json"

[mission.params]
location = "Melbourne"
"#,
    );

    let output = run_brr_in(&dir, &["--log-format", "json"]);

    assert_eq!(output.status.code(), Some(64));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    let event: Value = serde_json::from_str(stderr.trim()).unwrap();
    assert_eq!(event["category"], "config_invalid");
}
