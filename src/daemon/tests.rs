use super::*;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration as StdDuration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

static TEST_LOCK: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

#[tokio::test(flavor = "current_thread")]
async fn rescue_abort_cleans_up_registry_and_preserves_watch_history() {
    let _serial = TEST_LOCK.lock().await;
    let _spawn_hook = SpawnHookGuard::set(spawn_fake_child);
    let home = unique_home();

    let daemon = tokio::spawn(run_in(home.clone()));
    wait_for_socket(&home).await;

    let sock = socket_path_in(&home);
    let mission = "solar-wind";

    let launched = send_command(
        &sock,
        &Command::Launch {
            wasm: "fake.wasm".to_string(),
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

    wait_for_status_len(&sock, 1).await;
    wait_for_event_file(&home, mission, "mission boot").await;

    let watched = watch_first_response(&sock, mission).await;
    assert!(matches!(
        watched,
        Response::Event { mission: ref name, ref line }
            if name == mission && line.contains("\"message\":\"mission boot\"")
    ));

    let rescue = send_command(
        &sock,
        &Command::Rescue {
            mission: mission.to_string(),
            action: RescueAction::Abort,
            reason: "operator requested stop".to_string(),
        },
    )
    .await;
    assert!(matches!(rescue, Response::Ok { mission: ref name } if name == mission));

    wait_for_status_len(&sock, 0).await;

    let historical = watch_first_response(&sock, mission).await;
    assert!(matches!(
        historical,
        Response::Event { mission: ref name, ref line }
            if name == mission && line.contains("\"message\":\"mission boot\"")
    ));

    let relaunched = send_command(
        &sock,
        &Command::Launch {
            wasm: "fake.wasm".to_string(),
            name: Some(mission.to_string()),
            env: HashMap::new(),
            params: None,
        },
    )
    .await;
    assert!(matches!(
        relaunched,
        Response::Launched { mission: ref name } if name == mission
    ));

    let abort = send_command(
        &sock,
        &Command::Abort {
            mission: mission.to_string(),
            reason: "shutdown".to_string(),
        },
    )
    .await;
    assert!(matches!(abort, Response::Ok { mission: ref name } if name == mission));
    wait_for_status_len(&sock, 0).await;

    let bye = send_command(&sock, &Command::Shutdown).await;
    assert!(matches!(bye, Response::Bye));
    daemon
        .await
        .expect("daemon task panicked")
        .expect("daemon returned error");
    let _ = std::fs::remove_dir_all(home);
}

#[tokio::test(flavor = "current_thread")]
async fn daemon_refuses_to_take_over_an_active_socket() {
    let _serial = TEST_LOCK.lock().await;
    let _spawn_hook = SpawnHookGuard::set(spawn_fake_child);
    let home = unique_home();

    let daemon = tokio::spawn(run_in(home.clone()));
    wait_for_socket(&home).await;

    let error = run_in(home.clone())
        .await
        .expect_err("second daemon should refuse active socket");
    assert!(format!("{error:#}").contains("already running"));

    let sock = socket_path_in(&home);
    let bye = send_command(&sock, &Command::Shutdown).await;
    assert!(matches!(bye, Response::Bye));
    daemon
        .await
        .expect("daemon task panicked")
        .expect("daemon returned error");
    let _ = std::fs::remove_dir_all(home);
}

#[tokio::test(flavor = "current_thread")]
async fn relaunch_resets_watch_history_for_reused_mission_names() {
    let _serial = TEST_LOCK.lock().await;
    let _spawn_hook = SpawnHookGuard::set(spawn_fake_child);
    let home = unique_home();

    let daemon = tokio::spawn(run_in(home.clone()));
    wait_for_socket(&home).await;

    let sock = socket_path_in(&home);
    let mission = "solar-wind";

    let first = send_command(
        &sock,
        &Command::Launch {
            wasm: "fake.wasm".to_string(),
            name: Some(mission.to_string()),
            env: HashMap::from([("MARKER".to_string(), "first boot".to_string())]),
            params: None,
        },
    )
    .await;
    assert!(matches!(first, Response::Launched { mission: ref name } if name == mission));
    wait_for_event_file(&home, mission, "first boot").await;

    let abort = send_command(
        &sock,
        &Command::Abort {
            mission: mission.to_string(),
            reason: "test reset".to_string(),
        },
    )
    .await;
    assert!(matches!(abort, Response::Ok { mission: ref name } if name == mission));
    wait_for_status_len(&sock, 0).await;

    let second = send_command(
        &sock,
        &Command::Launch {
            wasm: "fake.wasm".to_string(),
            name: Some(mission.to_string()),
            env: HashMap::from([("MARKER".to_string(), "second boot".to_string())]),
            params: None,
        },
    )
    .await;
    assert!(matches!(second, Response::Launched { mission: ref name } if name == mission));
    wait_for_event_file(&home, mission, "second boot").await;

    let watched = watch_first_response(&sock, mission).await;
    assert!(matches!(
        watched,
        Response::Event { mission: ref name, ref line }
            if name == mission && line.contains("\"message\":\"second boot\"")
    ));

    let events = std::fs::read_to_string(mission_events_file(&home, mission))
        .expect("read mission events file");
    assert!(!events.contains("first boot"));
    assert!(events.contains("second boot"));

    let bye = send_command(&sock, &Command::Shutdown).await;
    assert!(matches!(bye, Response::Bye));
    daemon
        .await
        .expect("daemon task panicked")
        .expect("daemon returned error");
    let _ = std::fs::remove_dir_all(home);
}

struct SpawnHookGuard;

impl SpawnHookGuard {
    fn set(hook: SpawnChildHook) -> Self {
        *SPAWN_CHILD_HOOK.lock().expect("spawn hook mutex poisoned") = Some(hook);
        Self
    }
}

impl Drop for SpawnHookGuard {
    fn drop(&mut self) {
        *SPAWN_CHILD_HOOK.lock().expect("spawn hook mutex poisoned") = None;
    }
}

fn spawn_fake_child(
    _wasm: &str,
    env: &HashMap<String, String>,
    _params: Option<&str>,
    _override_retry_gate: bool,
) -> Result<tokio::process::Child> {
    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c")
        .arg(
            "printf '%s\n' \"{\\\"type\\\":\\\"log\\\",\\\"ts\\\":\\\"2026-04-21T00:00:00.000Z\\\",\\\"message\\\":\\\"${MARKER:-mission boot}\\\"}\"; \
             trap 'exit 0' TERM INT; \
             while :; do sleep 1; done",
        )
        .envs(env)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(false);
    Ok(cmd.spawn()?)
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

async fn wait_for_event_file(home: &Path, mission: &str, expected: &str) {
    let path = mission_events_file(home, mission);
    for _ in 0..120 {
        if let Ok(contents) = tokio::fs::read_to_string(&path).await
            && contents.contains(expected)
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!(
        "event file did not contain expected mission output '{}': {}",
        expected,
        path.display(),
    );
}

async fn wait_for_status_len(sock: &Path, expected: usize) {
    for _ in 0..120 {
        if let Response::Status { missions } = send_command(sock, &Command::Status).await
            && missions.len() == expected
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("daemon never reached expected mission count {expected}");
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
