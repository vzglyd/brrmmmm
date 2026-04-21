mod client;
mod mission;
mod protocol;
mod service;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Mutex, broadcast, mpsc};
use tokio::time::Duration;

use mission::{
    BROADCAST_CAPACITY, MAX_MISSIONS, MissionCleanup, MissionCtrl, MissionHandle, MissionState,
};
use protocol::{Command, Response};

pub(crate) use client::DaemonClient;
pub(crate) use protocol::{Command as DaemonCommand, RescueAction, Response as DaemonResponse};
pub(crate) use service::{
    daemon_install, daemon_restart, daemon_start, daemon_status, daemon_stop, daemon_uninstall,
};

pub(crate) fn brrmmmm_home() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".brrmmmm")
}

pub(crate) fn socket_path() -> PathBuf {
    socket_path_in(&brrmmmm_home())
}

fn socket_path_in(home: &Path) -> PathBuf {
    home.join("daemon.sock")
}

fn mission_dir(home: &Path, mission: &str) -> PathBuf {
    home.join("missions").join(mission)
}

fn mission_events_file(home: &Path, mission: &str) -> PathBuf {
    mission_dir(home, mission).join("events.ndjson")
}

struct Registry {
    missions: HashMap<String, MissionHandle>,
    taken_names: HashSet<String>,
}

pub(crate) async fn run() -> Result<()> {
    run_in(brrmmmm_home()).await
}

async fn run_in(home: PathBuf) -> Result<()> {
    let home = Arc::new(home);
    tokio::fs::create_dir_all(home.as_ref()).await?;
    tokio::fs::create_dir_all(home.join("missions")).await?;

    let pid_path = home.join("daemon.pid");
    tokio::fs::write(&pid_path, std::process::id().to_string()).await?;

    let sock_path = socket_path_in(home.as_ref());
    if sock_path.exists() {
        tokio::fs::remove_file(&sock_path).await?;
    }

    let listener = UnixListener::bind(&sock_path)?;
    let registry = Arc::new(Mutex::new(Registry {
        missions: HashMap::new(),
        taken_names: HashSet::new(),
    }));
    let (cleanup_tx, mut cleanup_rx) = mpsc::unbounded_channel::<String>();
    let cleanup_registry = Arc::clone(&registry);
    tokio::spawn(async move {
        while let Some(name) = cleanup_rx.recv().await {
            remove_mission(&cleanup_registry, &name).await;
        }
    });

    eprintln!("[brrmmmm daemon] listening on {}", sock_path.display());

    let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let shutdown_cell: Arc<Mutex<Option<tokio::sync::oneshot::Sender<()>>>> =
        Arc::new(Mutex::new(Some(shutdown_tx)));

    loop {
        tokio::select! {
            accept = listener.accept() => {
                match accept {
                    Ok((stream, _)) => {
                        let registry = Arc::clone(&registry);
                        let home = Arc::clone(&home);
                        let cell = Arc::clone(&shutdown_cell);
                        let cleanup_tx = cleanup_tx.clone();
                        tokio::spawn(handle_connection(stream, home, registry, cell, cleanup_tx));
                    }
                    Err(e) => eprintln!("[brrmmmm daemon] accept error: {e}"),
                }
            }
            _ = &mut shutdown_rx => {
                eprintln!("[brrmmmm daemon] shutting down");
                break;
            }
        }
    }

    let _ = tokio::fs::remove_file(&sock_path).await;
    let _ = tokio::fs::remove_file(&pid_path).await;
    Ok(())
}

async fn handle_connection(
    stream: UnixStream,
    home: Arc<PathBuf>,
    registry: Arc<Mutex<Registry>>,
    shutdown_cell: Arc<Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
    cleanup_tx: mpsc::UnboundedSender<String>,
) {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    while let Ok(Some(line)) = lines.next_line().await {
        let cmd: Command = match serde_json::from_str(&line) {
            Ok(c) => c,
            Err(e) => {
                let _ = write_resp(
                    &mut writer,
                    &Response::Error {
                        message: format!("parse error: {e}"),
                    },
                )
                .await;
                continue;
            }
        };

        match cmd {
            Command::Ping => {
                let _ = write_resp(&mut writer, &Response::Pong).await;
            }
            Command::Shutdown => {
                let _ = write_resp(&mut writer, &Response::Bye).await;
                let mut cell = shutdown_cell.lock().await;
                if let Some(tx) = cell.take() {
                    let _ = tx.send(());
                }
                return;
            }
            Command::Status => {
                let states = {
                    let reg = registry.lock().await;
                    reg.missions
                        .values()
                        .map(|handle| Arc::clone(&handle.state))
                        .collect::<Vec<_>>()
                };
                let mut summaries = Vec::with_capacity(states.len());
                for state in states {
                    summaries.push(state.lock().await.summary());
                }
                let _ = write_resp(
                    &mut writer,
                    &Response::Status {
                        missions: summaries,
                    },
                )
                .await;
            }
            Command::Launch {
                wasm,
                name,
                env,
                params,
            } => {
                let resp = cmd_launch(
                    &home,
                    &registry,
                    cleanup_tx.clone(),
                    wasm,
                    name,
                    env,
                    params,
                )
                .await;
                let _ = write_resp(&mut writer, &resp).await;
            }
            Command::Hold { mission, reason: _ } => {
                let resp = send_ctrl(&registry, &mission, MissionCtrl::Hold).await;
                let _ = write_resp(&mut writer, &resp).await;
            }
            Command::Resume { mission } => {
                let resp = send_ctrl(&registry, &mission, MissionCtrl::Resume).await;
                let _ = write_resp(&mut writer, &resp).await;
            }
            Command::Abort { mission, reason: _ } => {
                let resp = send_ctrl(&registry, &mission, MissionCtrl::Abort).await;
                let _ = write_resp(&mut writer, &resp).await;
            }
            Command::Rescue {
                mission,
                action,
                reason: _,
            } => {
                let resp = send_ctrl(&registry, &mission, rescue_control(action)).await;
                let _ = write_resp(&mut writer, &resp).await;
            }
            Command::Watch { mission } => {
                cmd_watch(home.as_ref(), &registry, &mission, &mut writer).await;
                return;
            }
        }
    }
}

async fn cmd_launch(
    home: &Path,
    registry: &Arc<Mutex<Registry>>,
    cleanup_tx: mpsc::UnboundedSender<String>,
    wasm: String,
    name: Option<String>,
    env: HashMap<String, String>,
    params: Option<String>,
) -> Response {
    let mut reg = registry.lock().await;

    if reg.missions.len() >= MAX_MISSIONS {
        return Response::Full {
            message: format!("daemon at {MAX_MISSIONS} mission capacity"),
        };
    }

    let mission_name = if let Some(n) = name {
        if reg.taken_names.contains(&n) {
            return Response::Error {
                message: format!("mission name '{n}' already taken"),
            };
        }
        n
    } else {
        match crate::names::generate_mission_name(&reg.taken_names) {
            Some(n) => n,
            None => {
                return Response::Error {
                    message: "could not generate a unique mission name".into(),
                };
            }
        }
    };

    let missions_dir = mission_dir(home, &mission_name);
    if let Err(e) = std::fs::create_dir_all(&missions_dir) {
        return Response::Error {
            message: format!("create mission dir: {e}"),
        };
    }

    let (event_tx, _) = broadcast::channel::<String>(BROADCAST_CAPACITY);
    let (ctrl_tx, ctrl_rx) = mpsc::channel::<MissionCtrl>(32);

    let state = Arc::new(Mutex::new(MissionState {
        name: mission_name.clone(),
        wasm: wasm.clone(),
        held: false,
        terminal: false,
        phase: "idle".into(),
        cycles: 0,
        pid: None,
    }));

    reg.missions.insert(
        mission_name.clone(),
        MissionHandle {
            state: Arc::clone(&state),
            event_tx: event_tx.clone(),
            ctrl_tx,
        },
    );
    reg.taken_names.insert(mission_name.clone());
    drop(reg);

    tokio::spawn(mission_loop(MissionLoopArgs {
        home: home.to_path_buf(),
        name: mission_name.clone(),
        wasm,
        env,
        params,
        state,
        event_tx,
        ctrl_rx,
        cleanup_tx,
    }));

    Response::Launched {
        mission: mission_name,
    }
}

fn rescue_control(action: RescueAction) -> MissionCtrl {
    match action {
        RescueAction::Retry => MissionCtrl::Retry,
        RescueAction::Abort => MissionCtrl::Abort,
    }
}

async fn send_ctrl(registry: &Arc<Mutex<Registry>>, mission: &str, ctrl: MissionCtrl) -> Response {
    let ctrl_tx = {
        let reg = registry.lock().await;
        reg.missions
            .get(mission)
            .map(|handle| handle.ctrl_tx.clone())
    };
    match ctrl_tx {
        None => Response::Error {
            message: format!("no mission named '{mission}'"),
        },
        Some(ctrl_tx) => {
            if ctrl_tx.send(ctrl).await.is_err() {
                Response::Error {
                    message: format!("mission '{mission}' is not accepting commands"),
                }
            } else {
                Response::Ok {
                    mission: mission.to_string(),
                }
            }
        }
    }
}

async fn cmd_watch(
    home: &Path,
    registry: &Arc<Mutex<Registry>>,
    mission: &str,
    writer: &mut (impl AsyncWrite + Unpin),
) {
    let event_rx = {
        let reg = registry.lock().await;
        reg.missions
            .get(mission)
            .map(|handle| handle.event_tx.subscribe())
    };
    let events_file = mission_events_file(home, mission);
    if event_rx.is_none() && !tokio::fs::try_exists(&events_file).await.unwrap_or(false) {
        let _ = write_resp(
            writer,
            &Response::Error {
                message: format!("no mission named '{mission}'"),
            },
        )
        .await;
        return;
    }

    let name = mission.to_string();

    // Drain historical events from file first, then stream live events.
    if let Ok(content) = tokio::fs::read_to_string(&events_file).await {
        for line in content.lines() {
            if line.is_empty() {
                continue;
            }
            if write_resp(
                writer,
                &Response::Event {
                    mission: name.clone(),
                    line: line.to_string(),
                },
            )
            .await
            .is_err()
            {
                return;
            }
        }
    }

    let Some(mut event_rx) = event_rx else {
        return;
    };

    loop {
        match event_rx.recv().await {
            Ok(line) => {
                if write_resp(
                    writer,
                    &Response::Event {
                        mission: name.clone(),
                        line,
                    },
                )
                .await
                .is_err()
                {
                    return;
                }
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                eprintln!("[brrmmmm daemon] watch '{name}' lagged by {n} events");
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

async fn write_resp(writer: &mut (impl AsyncWrite + Unpin), resp: &Response) -> Result<()> {
    let json = serde_json::to_string(resp)?;
    writer.write_all(format!("{json}\n").as_bytes()).await?;
    Ok(())
}

async fn remove_mission(registry: &Arc<Mutex<Registry>>, name: &str) {
    let mut reg = registry.lock().await;
    reg.missions.remove(name);
    reg.taken_names.remove(name);
}

// ── Per-mission child process management ─────────────────────────────────────

struct MissionLoopArgs {
    home: PathBuf,
    name: String,
    wasm: String,
    env: HashMap<String, String>,
    params: Option<String>,
    state: Arc<Mutex<MissionState>>,
    event_tx: broadcast::Sender<String>,
    ctrl_rx: mpsc::Receiver<MissionCtrl>,
    cleanup_tx: mpsc::UnboundedSender<String>,
}

async fn mission_loop(args: MissionLoopArgs) {
    let MissionLoopArgs {
        home,
        name,
        wasm,
        env,
        params,
        state,
        event_tx,
        mut ctrl_rx,
        cleanup_tx,
    } = args;
    let _cleanup = MissionCleanup::new(name.clone(), cleanup_tx);
    let events_file = mission_events_file(&home, &name);
    let mut override_retry_gate = false;

    loop {
        // If terminal, exit loop.
        {
            if state.lock().await.terminal {
                return;
            }
        }

        // If held without a running process, wait for Resume or Abort.
        {
            let held = state.lock().await.held;
            if held {
                loop {
                    match ctrl_rx.recv().await {
                        Some(MissionCtrl::Resume) => {
                            state.lock().await.held = false;
                            break;
                        }
                        Some(MissionCtrl::Abort) => {
                            let mut state = state.lock().await;
                            state.terminal = true;
                            state.held = false;
                            return;
                        }
                        Some(MissionCtrl::Retry) => {
                            override_retry_gate = true;
                            state.lock().await.held = false;
                            break;
                        }
                        Some(MissionCtrl::Hold) => {} // already held
                        None => return,
                    }
                }
            }
        }

        let mut child = match spawn_child(&wasm, &env, params.as_deref(), override_retry_gate) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[brrmmmm daemon] spawn '{name}' failed: {e}");
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        };
        override_retry_gate = false;

        let pid = child.id().unwrap_or(0);
        state.lock().await.pid = Some(pid);

        if let Some(stdout) = child.stdout.take() {
            tokio::spawn(pipe_events(
                stdout,
                events_file.clone(),
                event_tx.clone(),
                Arc::clone(&state),
            ));
        }

        // Wait for child exit or operator control command.
        loop {
            tokio::select! {
                _ = child.wait() => {
                    state.lock().await.pid = None;
                    break;
                }
                ctrl = ctrl_rx.recv() => {
                    match ctrl {
                        Some(MissionCtrl::Hold) => {
                            send_signal(pid, "STOP");
                            state.lock().await.held = true;
                        }
                        Some(MissionCtrl::Resume) => {
                            send_signal(pid, "CONT");
                            state.lock().await.held = false;
                        }
                        Some(MissionCtrl::Abort) => {
                            let _ = child.start_kill();
                            {
                                let mut state = state.lock().await;
                                state.terminal = true;
                                state.held = false;
                            }
                            let _ = child.wait().await;
                            state.lock().await.pid = None;
                            return;
                        }
                        Some(MissionCtrl::Retry) => {
                            override_retry_gate = true;
                            state.lock().await.held = false;
                            let _ = child.start_kill();
                            // child.wait() will fire on next loop iteration
                        }
                        None => {
                            // ctrl channel closed; daemon shutting down
                            let _ = child.start_kill();
                            return;
                        }
                    }
                }
            }
        }

        if state.lock().await.terminal {
            return;
        }

        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

async fn pipe_events(
    stdout: tokio::process::ChildStdout,
    events_file: PathBuf,
    event_tx: broadcast::Sender<String>,
    state: Arc<Mutex<MissionState>>,
) {
    use tokio::io::AsyncWriteExt as _;

    let mut lines = BufReader::new(stdout).lines();
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&events_file)
        .await
        .ok();

    while let Ok(Some(line)) = lines.next_line().await {
        if let Some(ref mut f) = file {
            let _ = f.write_all(format!("{line}\n").as_bytes()).await;
        }
        update_state_from_event(&state, &line).await;
        let _ = event_tx.send(line);
    }
}

async fn update_state_from_event(state: &Arc<Mutex<MissionState>>, line: &str) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
        return;
    };
    let event_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    let mut s = state.lock().await;
    match event_type {
        "phase" => {
            if let Some(phase) = v.get("phase").and_then(|p| p.as_str()) {
                s.phase = phase.to_string();
            }
        }
        "mission_outcome" => {
            s.cycles += 1;
            s.phase = "idle".into();
        }
        _ => {}
    }
}

#[cfg(test)]
type SpawnChildHook =
    fn(&str, &HashMap<String, String>, Option<&str>, bool) -> Result<tokio::process::Child>;

#[cfg(test)]
static SPAWN_CHILD_HOOK: std::sync::Mutex<Option<SpawnChildHook>> = std::sync::Mutex::new(None);

fn spawn_child(
    wasm: &str,
    env: &HashMap<String, String>,
    params: Option<&str>,
    override_retry_gate: bool,
) -> Result<tokio::process::Child> {
    #[cfg(test)]
    {
        if let Some(hook) = *SPAWN_CHILD_HOOK.lock().expect("spawn hook mutex poisoned") {
            return hook(wasm, env, params, override_retry_gate);
        }
    }

    let exe = std::env::current_exe()?;
    let mut cmd = tokio::process::Command::new(&exe);
    cmd.arg("run")
        .arg(wasm)
        .arg("--events")
        .arg("--no-log-channel")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(false);

    for (k, v) in env {
        cmd.arg("-e").arg(format!("{k}={v}"));
    }
    if let Some(p) = params {
        cmd.arg("-j").arg(p);
    }
    if override_retry_gate {
        cmd.arg("--override-retry-gate");
    }

    Ok(cmd.spawn()?)
}

fn send_signal(pid: u32, sig: &str) {
    let _ = std::process::Command::new("kill")
        .arg(format!("-{sig}"))
        .arg(pid.to_string())
        .status();
}

#[cfg(test)]
mod tests {
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
        wait_for_event_file(&home, mission).await;

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
        _env: &HashMap<String, String>,
        _params: Option<&str>,
        _override_retry_gate: bool,
    ) -> Result<tokio::process::Child> {
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c")
            .arg(
                "printf '%s\n' '{\"type\":\"log\",\"ts\":\"2026-04-21T00:00:00.000Z\",\"message\":\"mission boot\"}'; \
                 trap 'exit 0' TERM INT; \
                 while :; do sleep 1; done",
            )
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

    async fn wait_for_event_file(home: &Path, mission: &str) {
        let path = mission_events_file(home, mission);
        for _ in 0..120 {
            if let Ok(contents) = tokio::fs::read_to_string(&path).await
                && contents.contains("mission boot")
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!(
            "event file did not contain mission output: {}",
            path.display()
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
}
