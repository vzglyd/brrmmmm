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
    BROADCAST_CAPACITY, MAX_MISSIONS, MissionCleanup, MissionCtrl, MissionEvent, MissionHandle,
    MissionHistory, MissionState,
};
use protocol::{Command, Response};

pub use client::DaemonClient;
pub use protocol::{Command as DaemonCommand, RescueAction, Response as DaemonResponse};
pub use service::{
    daemon_install, daemon_restart, daemon_start, daemon_status, daemon_stop, daemon_uninstall,
};

pub fn brrmmmm_home() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".brrmmmm")
}

pub fn socket_path() -> PathBuf {
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

fn prepare_mission_storage(home: &Path, mission: &str) -> Result<()> {
    let dir = mission_dir(home, mission);
    std::fs::create_dir_all(&dir)?;

    let events_file = mission_events_file(home, mission);
    if events_file.exists() {
        std::fs::remove_file(&events_file)?;
    }

    Ok(())
}

struct Registry {
    missions: HashMap<String, MissionHandle>,
    taken_names: HashSet<String>,
}

pub async fn run() -> Result<()> {
    run_in(brrmmmm_home()).await
}

async fn run_in(home: PathBuf) -> Result<()> {
    let home = Arc::new(home);
    tokio::fs::create_dir_all(home.as_ref()).await?;
    tokio::fs::create_dir_all(home.join("missions")).await?;

    let pid_path = home.join("daemon.pid");
    tokio::fs::write(&pid_path, std::process::id().to_string()).await?;

    let sock_path = socket_path_in(home.as_ref());
    let listener = prepare_listener(&sock_path).await?;
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
    let shutdown_signal = daemon_shutdown_signal();
    tokio::pin!(shutdown_signal);

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
            () = &mut shutdown_signal => {
                eprintln!("[brrmmmm daemon] received shutdown signal");
                break;
            }
        }
    }

    shutdown_missions(&registry).await;
    let _ = tokio::fs::remove_file(&sock_path).await;
    let _ = tokio::fs::remove_file(&pid_path).await;
    Ok(())
}

async fn prepare_listener(sock_path: &Path) -> Result<UnixListener> {
    if tokio::fs::try_exists(sock_path).await.unwrap_or(false) {
        if daemon_is_responding(sock_path).await {
            anyhow::bail!(
                "brrmmmm daemon is already running at {}",
                sock_path.display()
            );
        }
        tokio::fs::remove_file(sock_path).await?;
    }
    Ok(UnixListener::bind(sock_path)?)
}

async fn daemon_is_responding(sock_path: &Path) -> bool {
    let Ok(mut client) = DaemonClient::connect(sock_path).await else {
        return false;
    };
    matches!(client.send(&Command::Ping).await, Ok(Response::Pong))
}

async fn daemon_shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        match signal(SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = terminate.recv() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }

    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

async fn shutdown_missions(registry: &Arc<Mutex<Registry>>) {
    let controls = {
        let reg = registry.lock().await;
        reg.missions
            .iter()
            .map(|(name, handle)| (name.clone(), handle.ctrl_tx.clone()))
            .collect::<Vec<_>>()
    };

    for (name, ctrl_tx) in controls {
        if ctrl_tx.send(MissionCtrl::Abort).await.is_err() {
            eprintln!("[brrmmmm daemon] failed to stop mission '{name}' during shutdown");
        }
    }

    for _ in 0..100 {
        if registry.lock().await.missions.is_empty() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    eprintln!("[brrmmmm daemon] shutdown timed out waiting for missions to stop");
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
                let tx = {
                    let mut cell = shutdown_cell.lock().await;
                    cell.take()
                };
                if let Some(tx) = tx {
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

    if let Err(e) = prepare_mission_storage(home, &mission_name) {
        return Response::Error {
            message: format!("prepare mission storage: {e}"),
        };
    }

    let (event_tx, _) = broadcast::channel::<MissionEvent>(BROADCAST_CAPACITY);
    let (ctrl_tx, ctrl_rx) = mpsc::channel::<MissionCtrl>(32);
    let history = Arc::new(Mutex::new(MissionHistory::default()));

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
            history: Arc::clone(&history),
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
        history,
        event_tx,
        ctrl_rx,
        cleanup_tx,
    }));

    Response::Launched {
        mission: mission_name,
    }
}

const fn rescue_control(action: RescueAction) -> MissionCtrl {
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
    let active_watch = {
        let reg = registry.lock().await;
        reg.missions
            .get(mission)
            .map(|handle| (Arc::clone(&handle.history), handle.event_tx.subscribe()))
    };
    let events_file = mission_events_file(home, mission);
    if active_watch.is_none() && !tokio::fs::try_exists(&events_file).await.unwrap_or(false) {
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

    if let Some((history, mut event_rx)) = active_watch {
        let (snapshot, cutoff_seq) = history.lock().await.snapshot();
        for event in snapshot {
            if write_resp(
                writer,
                &Response::Event {
                    mission: name.clone(),
                    line: event.line,
                },
            )
            .await
            .is_err()
            {
                return;
            }
        }

        loop {
            match event_rx.recv().await {
                Ok(event) => {
                    if event.seq < cutoff_seq {
                        continue;
                    }
                    if write_resp(
                        writer,
                        &Response::Event {
                            mission: name.clone(),
                            line: event.line,
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
        return;
    }

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
    history: Arc<Mutex<MissionHistory>>,
    event_tx: broadcast::Sender<MissionEvent>,
    ctrl_rx: mpsc::Receiver<MissionCtrl>,
    cleanup_tx: mpsc::UnboundedSender<String>,
}

enum MissionChildDirective {
    Continue,
    Abort,
}

async fn mission_loop(args: MissionLoopArgs) {
    let MissionLoopArgs {
        home,
        name,
        wasm,
        env,
        params,
        state,
        history,
        event_tx,
        mut ctrl_rx,
        cleanup_tx,
    } = args;
    let _cleanup = MissionCleanup::new(name.clone(), cleanup_tx);
    let events_file = mission_events_file(&home, &name);
    let mut override_retry_gate = false;

    loop {
        if state.lock().await.terminal {
            return;
        }

        if wait_while_held(&state, &mut ctrl_rx, &mut override_retry_gate).await {
            return;
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
                Arc::clone(&history),
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
                    match handle_child_control(
                        ctrl,
                        pid,
                        &state,
                        &mut child,
                        &mut override_retry_gate,
                    )
                    .await {
                        MissionChildDirective::Continue => {}
                        MissionChildDirective::Abort => return,
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

async fn wait_while_held(
    state: &Arc<Mutex<MissionState>>,
    ctrl_rx: &mut mpsc::Receiver<MissionCtrl>,
    override_retry_gate: &mut bool,
) -> bool {
    if !state.lock().await.held {
        return false;
    }

    loop {
        match ctrl_rx.recv().await {
            Some(MissionCtrl::Resume) => {
                state.lock().await.held = false;
                return false;
            }
            Some(MissionCtrl::Abort) => {
                let mut state = state.lock().await;
                state.terminal = true;
                state.held = false;
                drop(state);
                return true;
            }
            Some(MissionCtrl::Retry) => {
                *override_retry_gate = true;
                state.lock().await.held = false;
                return false;
            }
            Some(MissionCtrl::Hold) => {}
            None => return true,
        }
    }
}

async fn handle_child_control(
    ctrl: Option<MissionCtrl>,
    pid: u32,
    state: &Arc<Mutex<MissionState>>,
    child: &mut tokio::process::Child,
    override_retry_gate: &mut bool,
) -> MissionChildDirective {
    match ctrl {
        Some(MissionCtrl::Hold) => {
            send_signal(pid, "STOP");
            state.lock().await.held = true;
            MissionChildDirective::Continue
        }
        Some(MissionCtrl::Resume) => {
            send_signal(pid, "CONT");
            state.lock().await.held = false;
            MissionChildDirective::Continue
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
            MissionChildDirective::Abort
        }
        Some(MissionCtrl::Retry) => {
            *override_retry_gate = true;
            state.lock().await.held = false;
            let _ = child.start_kill();
            MissionChildDirective::Continue
        }
        None => {
            let _ = child.start_kill();
            MissionChildDirective::Abort
        }
    }
}

async fn pipe_events(
    stdout: tokio::process::ChildStdout,
    events_file: PathBuf,
    history: Arc<Mutex<MissionHistory>>,
    event_tx: broadcast::Sender<MissionEvent>,
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
        let event = history.lock().await.append(line);
        let _ = event_tx.send(event);
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
        let hook = *SPAWN_CHILD_HOOK.lock().expect("spawn hook mutex poisoned");
        if let Some(hook) = hook {
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
        .kill_on_drop(true);

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
mod tests;
