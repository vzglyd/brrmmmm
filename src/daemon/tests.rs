use super::*;

use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::sync::{LazyLock, OnceLock};
use std::time::{Duration as StdDuration, SystemTime, UNIX_EPOCH};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

static TEST_LOCK: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

#[tokio::test(flavor = "current_thread")]
async fn launch_rejects_relative_wasm_paths() {
    let _serial = TEST_LOCK.lock().await;
    let home = unique_home();
    let _env = TestEnvGuard::new(&home);

    let daemon = tokio::spawn(run_in(home.clone()));
    wait_for_socket(&home).await;

    let sock = socket_path_in(&home);
    let launched = send_command(
        &sock,
        &Command::Launch {
            wasm: "missions/demo-weather/demo.wasm".to_string(),
            name: Some("solar-wind".to_string()),
            env: HashMap::new(),
            params: None,
        },
    )
    .await;
    assert!(matches!(
        launched,
        Response::Error { ref message }
            if message.contains("absolute WASM path")
    ));

    shutdown_daemon(&sock, daemon).await;
    let _ = std::fs::remove_dir_all(home);
}

#[tokio::test(flavor = "current_thread")]
async fn published_mission_is_scheduled_from_declared_poll_strategy() {
    let _serial = TEST_LOCK.lock().await;
    let home = unique_home();
    let _env = TestEnvGuard::new(&home);
    let wasm = deterministic_fixture_wasm();

    let daemon = tokio::spawn(run_in(home.clone()));
    wait_for_socket(&home).await;

    let sock = socket_path_in(&home);
    let mission = "steady-orbit";
    let launched = send_command(
        &sock,
        &Command::Launch {
            wasm: wasm.to_string_lossy().into_owned(),
            name: Some(mission.to_string()),
            env: HashMap::new(),
            params: None,
        },
    )
    .await;
    assert!(matches!(
        launched,
        Response::Launched { mission: ref name } if name == mission
    ));

    let summary = wait_for_mission_summary(&sock, mission, |summary| {
        summary.state == MissionSchedulerState::Scheduled
            && summary.last_outcome_status.as_deref() == Some("published")
    })
    .await;
    let last_run = summary.last_run_at_ms.expect("published run timestamp");
    let next_wake = summary.next_wake_at_ms.expect("scheduled wake time");
    assert!(
        next_wake >= last_run.saturating_add(59_000),
        "next wake should honor the 60s fixed interval, got last_run={last_run} next_wake={next_wake}"
    );
    assert_eq!(summary.phase, "publishing");

    let events = std::fs::read_to_string(mission_events_file(&home, mission))
        .expect("read mission events file");
    assert!(events.contains("\"type\":\"mission_outcome\""));
    assert!(events.contains("\"status\":\"published\""));

    let abort = send_command(
        &sock,
        &Command::Abort {
            mission: mission.to_string(),
            reason: "test shutdown".to_string(),
        },
    )
    .await;
    assert!(matches!(abort, Response::Ok { mission: ref name } if name == mission));
    wait_for_status_len(&sock, 0).await;

    shutdown_daemon(&sock, daemon).await;
    let _ = std::fs::remove_dir_all(home);
}

#[tokio::test(flavor = "current_thread")]
async fn operator_action_mission_waits_for_rescue_and_retry_relaunches_it() {
    let _serial = TEST_LOCK.lock().await;
    let home = unique_home();
    let _env = TestEnvGuard::new(&home);
    let wasm = operator_action_fixture_wasm();

    let daemon = tokio::spawn(run_in(home.clone()));
    wait_for_socket(&home).await;

    let sock = socket_path_in(&home);
    let mission = "manual-rescue";
    let launched = send_command(
        &sock,
        &Command::Launch {
            wasm: wasm.to_string_lossy().into_owned(),
            name: Some(mission.to_string()),
            env: HashMap::new(),
            params: None,
        },
    )
    .await;
    assert!(matches!(
        launched,
        Response::Launched { mission: ref name } if name == mission
    ));

    let first = wait_for_mission_summary(&sock, mission, |summary| {
        summary.state == MissionSchedulerState::AwaitingOperator
            && summary.last_outcome_status.as_deref() == Some("operator_action_required")
    })
    .await;
    let first_cycles = first.cycles;

    let retry = send_command(
        &sock,
        &Command::Rescue {
            mission: mission.to_string(),
            action: RescueAction::Retry,
            reason: "retry immediately".to_string(),
        },
    )
    .await;
    assert!(matches!(retry, Response::Ok { mission: ref name } if name == mission));

    let relaunched = wait_for_mission_summary(&sock, mission, |summary| {
        summary.state == MissionSchedulerState::AwaitingOperator && summary.cycles > first_cycles
    })
    .await;
    assert!(relaunched.cycles > first_cycles);

    let abort = send_command(
        &sock,
        &Command::Rescue {
            mission: mission.to_string(),
            action: RescueAction::Abort,
            reason: "operator declined".to_string(),
        },
    )
    .await;
    assert!(matches!(abort, Response::Ok { mission: ref name } if name == mission));
    wait_for_status_len(&sock, 0).await;

    let watched = watch_first_response(&sock, mission).await;
    assert!(matches!(
        watched,
        Response::Event { mission: ref name, ref line }
            if name == mission && line.contains("\"type\":\"env_snapshot\"")
    ));

    shutdown_daemon(&sock, daemon).await;
    let _ = std::fs::remove_dir_all(home);
}

#[tokio::test(flavor = "current_thread")]
async fn watch_status_streams_live_mission_updates() {
    let _serial = TEST_LOCK.lock().await;
    let home = unique_home();
    let _env = TestEnvGuard::new(&home);
    let wasm = deterministic_fixture_wasm();

    let daemon = tokio::spawn(run_in(home.clone()));
    wait_for_socket(&home).await;

    let sock = socket_path_in(&home);
    let mut stream = UnixStream::connect(&sock)
        .await
        .expect("connect watch status socket");
    let command = serde_json::to_string(&Command::WatchStatus).expect("serialize watch status");
    stream
        .write_all(format!("{command}\n").as_bytes())
        .await
        .expect("write watch status command");
    let mut reader = BufReader::new(stream);

    let initial = read_status_response(&mut reader).await;
    assert!(initial.is_empty());

    let mission = "status-stream";
    let launched = send_command(
        &sock,
        &Command::Launch {
            wasm: wasm.to_string_lossy().into_owned(),
            name: Some(mission.to_string()),
            env: HashMap::new(),
            params: None,
        },
    )
    .await;
    assert!(matches!(
        launched,
        Response::Launched { mission: ref name } if name == mission
    ));

    let update = read_status_until(&mut reader, |missions| {
        missions.iter().any(|summary| summary.name == mission)
    })
    .await;
    assert!(update.iter().any(|summary| summary.name == mission));

    let abort = send_command(
        &sock,
        &Command::Abort {
            mission: mission.to_string(),
            reason: "status watcher cleanup".to_string(),
        },
    )
    .await;
    assert!(matches!(abort, Response::Ok { mission: ref name } if name == mission));
    let empty = read_status_until(&mut reader, |missions| missions.is_empty()).await;
    assert!(empty.is_empty());

    shutdown_daemon(&sock, daemon).await;
    let _ = std::fs::remove_dir_all(home);
}

struct TestEnvGuard {
    identity_dir: Option<std::ffi::OsString>,
    state_dir: Option<std::ffi::OsString>,
    attestation: Option<std::ffi::OsString>,
}

impl TestEnvGuard {
    fn new(home: &Path) -> Self {
        let guard = Self {
            identity_dir: std::env::var_os("BRRMMMM_IDENTITY_DIR"),
            state_dir: std::env::var_os("BRRMMMM_STATE_DIR"),
            attestation: std::env::var_os("BRRMMMM_ATTESTATION"),
        };

        unsafe {
            std::env::set_var("BRRMMMM_IDENTITY_DIR", home.join(".identity"));
            std::env::set_var("BRRMMMM_STATE_DIR", home.join(".state"));
            std::env::set_var("BRRMMMM_ATTESTATION", "off");
        }

        guard
    }
}

impl Drop for TestEnvGuard {
    fn drop(&mut self) {
        restore_env("BRRMMMM_IDENTITY_DIR", self.identity_dir.take());
        restore_env("BRRMMMM_STATE_DIR", self.state_dir.take());
        restore_env("BRRMMMM_ATTESTATION", self.attestation.take());
    }
}

fn restore_env(key: &str, value: Option<std::ffi::OsString>) {
    unsafe {
        match value {
            Some(value) => std::env::set_var(key, value),
            None => std::env::remove_var(key),
        }
    }
}

fn deterministic_fixture_wasm() -> PathBuf {
    static WASM: OnceLock<PathBuf> = OnceLock::new();
    WASM.get_or_init(|| {
        build_fixture_wasm(
            "fixtures/deterministic-sidecar/Cargo.toml",
            "target/test-fixtures/deterministic-sidecar",
            "wasm32-wasip1/release/deterministic_sidecar.wasm",
        )
    })
    .clone()
}

fn operator_action_fixture_wasm() -> PathBuf {
    static WASM: OnceLock<PathBuf> = OnceLock::new();
    WASM.get_or_init(|| {
        build_fixture_wasm(
            "fixtures/operator-action-module/Cargo.toml",
            "target/test-fixtures/operator-action-module",
            "wasm32-wasip1/release/operator_action_module.wasm",
        )
    })
    .clone()
}

fn build_fixture_wasm(manifest_rel: &str, target_rel: &str, artifact_rel: &str) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let manifest = root.join(manifest_rel);
    let target_dir = root.join(target_rel);
    let status = ProcessCommand::new("cargo")
        .args([
            "build",
            "--manifest-path",
            manifest.to_str().expect("manifest path utf8"),
            "--target",
            "wasm32-wasip1",
            "--release",
        ])
        .env("CARGO_TARGET_DIR", &target_dir)
        .status()
        .expect("failed to build fixture wasm");
    assert!(
        status.success(),
        "fixture build failed for {}",
        manifest.display()
    );
    target_dir.join(artifact_rel)
}

fn unique_home() -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| StdDuration::from_secs(0))
        .as_nanos();
    std::env::temp_dir().join(format!(
        "brrmmmm-daemon-test-{}-{stamp}",
        std::process::id()
    ))
}

async fn wait_for_socket(home: &Path) {
    let sock = socket_path_in(home);
    for _ in 0..120 {
        if tokio::fs::try_exists(&sock).await.unwrap_or(false) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("socket did not appear at {}", sock.display());
}

async fn wait_for_status_len(sock: &Path, expected: usize) {
    for _ in 0..240 {
        if let Response::Status { missions } = send_command(sock, &Command::Status).await
            && missions.len() == expected
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("daemon never reached expected mission count {expected}");
}

async fn wait_for_mission_summary(
    sock: &Path,
    mission: &str,
    predicate: impl Fn(&MissionSummary) -> bool,
) -> MissionSummary {
    for _ in 0..480 {
        if let Response::Status { missions } = send_command(sock, &Command::Status).await
            && let Some(summary) = missions.into_iter().find(|summary| summary.name == mission)
            && predicate(&summary)
        {
            return summary;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("mission '{mission}' never reached expected summary state");
}

async fn send_command(sock: &Path, cmd: &Command) -> Response {
    let mut client = DaemonClient::connect(sock)
        .await
        .expect("connect daemon client");
    client.send(cmd).await.expect("send daemon command")
}

async fn watch_first_response(sock: &Path, mission: &str) -> Response {
    let mut stream = UnixStream::connect(sock)
        .await
        .expect("connect watch socket");
    let command = serde_json::to_string(&Command::Watch {
        mission: mission.to_string(),
    })
    .expect("serialize watch command");
    stream
        .write_all(format!("{command}\n").as_bytes())
        .await
        .expect("write watch command");
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    let count = reader
        .read_line(&mut line)
        .await
        .expect("read watch response");
    assert!(count > 0, "watch returned no data");
    serde_json::from_str(line.trim_end()).expect("parse watch response")
}

async fn read_status_response(reader: &mut BufReader<UnixStream>) -> Vec<MissionSummary> {
    let mut line = String::new();
    let count = reader
        .read_line(&mut line)
        .await
        .expect("read status watch response");
    assert!(count > 0, "status watch returned no data");
    match serde_json::from_str::<Response>(line.trim_end()).expect("parse status response") {
        Response::Status { missions } => missions,
        other => panic!("expected status response, got {other:?}"),
    }
}

async fn read_status_until(
    reader: &mut BufReader<UnixStream>,
    predicate: impl Fn(&[MissionSummary]) -> bool,
) -> Vec<MissionSummary> {
    for _ in 0..40 {
        let missions = read_status_response(reader).await;
        if predicate(&missions) {
            return missions;
        }
    }
    panic!("status watch never produced the expected update");
}

async fn shutdown_daemon(sock: &Path, daemon: tokio::task::JoinHandle<Result<()>>) {
    let bye = send_command(sock, &Command::Shutdown).await;
    assert!(matches!(bye, Response::Bye));
    daemon
        .await
        .expect("daemon task panicked")
        .expect("daemon returned error");
}
