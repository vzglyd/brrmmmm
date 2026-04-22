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
use tokio::sync::{Mutex, broadcast, mpsc, watch};
use tokio::time::{Duration, Instant};

use brrmmmm::abi::{MissionOutcomeStatus, MissionPhase, NextAttemptPolicy, PollStrategy};
use brrmmmm::config::Config;
use brrmmmm::controller::{MissionCompletion, MissionController};
use brrmmmm::events::{EnvVarStatus, Event, EventSink, now_ms, now_ts};

use mission::{
    BROADCAST_CAPACITY, MAX_MISSIONS, MissionCleanup, MissionCtrl, MissionEvent, MissionHandle,
    MissionHistory, MissionState,
};
use protocol::{Command, MissionSchedulerState, MissionSummary, Response};

pub use client::DaemonClient;
pub use protocol::{
    Command as DaemonCommand, MissionSchedulerState as DaemonMissionSchedulerState, RescueAction,
    Response as DaemonResponse,
};
pub use service::{
    daemon_install, daemon_restart, daemon_start, daemon_status, daemon_stop, daemon_uninstall,
    is_service_not_installed,
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

#[derive(Clone)]
struct MissionLaunchConfig {
    wasm: String,
    env: HashMap<String, String>,
    params: Option<String>,
}

struct MissionLoopArgs {
    home: PathBuf,
    name: String,
    launch: MissionLaunchConfig,
    state: Arc<Mutex<MissionState>>,
    history: Arc<Mutex<MissionHistory>>,
    event_tx: broadcast::Sender<MissionEvent>,
    ctrl_rx: mpsc::Receiver<MissionCtrl>,
    cleanup_tx: mpsc::UnboundedSender<String>,
    registry: Arc<Mutex<Registry>>,
    status_tx: watch::Sender<Vec<MissionSummary>>,
    config: Arc<Config>,
}

struct LaunchContext<'a> {
    home: &'a Path,
    registry: &'a Arc<Mutex<Registry>>,
    status_tx: &'a watch::Sender<Vec<MissionSummary>>,
    config: &'a Config,
    cleanup_tx: mpsc::UnboundedSender<String>,
}

#[derive(Clone)]
enum MissionLoopMode {
    Launch {
        override_retry_gate: bool,
    },
    Scheduled {
        wake_at_ms: u64,
        override_retry_gate: bool,
    },
    Idle,
    AwaitingChange,
    AwaitingOperator,
    TerminalFailure,
}

enum MissionTransition {
    Continue(MissionLoopMode),
    Hold(MissionLoopMode),
    Abort,
}

struct MissionRecorder {
    file: Option<tokio::fs::File>,
    state: Arc<Mutex<MissionState>>,
    history: Arc<Mutex<MissionHistory>>,
    event_tx: broadcast::Sender<MissionEvent>,
    registry: Arc<Mutex<Registry>>,
    status_tx: watch::Sender<Vec<MissionSummary>>,
}

struct AttemptContext<'a> {
    mission_name: &'a str,
    state: &'a Arc<Mutex<MissionState>>,
    registry: &'a Arc<Mutex<Registry>>,
    status_tx: &'a watch::Sender<Vec<MissionSummary>>,
    ctrl_rx: &'a mut mpsc::Receiver<MissionCtrl>,
    recorder: &'a mut MissionRecorder,
    config: &'a Config,
}

impl MissionRecorder {
    async fn open(
        events_file: &Path,
        state: Arc<Mutex<MissionState>>,
        history: Arc<Mutex<MissionHistory>>,
        event_tx: broadcast::Sender<MissionEvent>,
        registry: Arc<Mutex<Registry>>,
        status_tx: watch::Sender<Vec<MissionSummary>>,
    ) -> Self {
        let file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(events_file)
            .await
            .ok();

        Self {
            file,
            state,
            history,
            event_tx,
            registry,
            status_tx,
        }
    }

    async fn record(&mut self, event: Event) {
        let Ok(line) = serde_json::to_string(&event) else {
            return;
        };

        if let Some(file) = self.file.as_mut()
            && file
                .write_all(format!("{line}\n").as_bytes())
                .await
                .is_err()
        {
            self.file = None;
        }

        update_mission_state(&self.state, &self.registry, &self.status_tx, |state| {
            apply_runtime_event(state, &event);
        })
        .await;

        let history_event = self.history.lock().await.append(line);
        let _ = self.event_tx.send(history_event);
    }

    async fn log(&mut self, message: impl Into<String>) {
        self.record(Event::Log {
            ts: now_ts(),
            message: message.into(),
        })
        .await;
    }
}

pub async fn run() -> Result<()> {
    run_in(brrmmmm_home()).await
}

async fn run_in(home: PathBuf) -> Result<()> {
    let config = Arc::new(Config::load()?);
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
    let (status_tx, _) = watch::channel::<Vec<MissionSummary>>(Vec::new());
    let (cleanup_tx, mut cleanup_rx) = mpsc::unbounded_channel::<String>();
    let cleanup_registry = Arc::clone(&registry);
    let cleanup_status_tx = status_tx.clone();
    tokio::spawn(async move {
        while let Some(name) = cleanup_rx.recv().await {
            remove_mission(&cleanup_registry, &cleanup_status_tx, &name).await;
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
                        let config = Arc::clone(&config);
                        let status_tx = status_tx.clone();
                        let cell = Arc::clone(&shutdown_cell);
                        let cleanup_tx = cleanup_tx.clone();
                        tokio::spawn(handle_connection(
                            stream,
                            home,
                            registry,
                            config,
                            status_tx,
                            cell,
                            cleanup_tx,
                        ));
                    }
                    Err(error) => eprintln!("[brrmmmm daemon] accept error: {error}"),
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
    config: Arc<Config>,
    status_tx: watch::Sender<Vec<MissionSummary>>,
    shutdown_cell: Arc<Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
    cleanup_tx: mpsc::UnboundedSender<String>,
) {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    while let Ok(Some(line)) = lines.next_line().await {
        let cmd: Command = match serde_json::from_str(&line) {
            Ok(command) => command,
            Err(error) => {
                let _ = write_resp(
                    &mut writer,
                    &Response::Error {
                        message: format!("parse error: {error}"),
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
                let missions = status_tx.borrow().clone();
                let _ = write_resp(&mut writer, &Response::Status { missions }).await;
            }
            Command::WatchStatus => {
                cmd_watch_status(status_tx.subscribe(), &mut writer).await;
                return;
            }
            Command::Launch {
                wasm,
                name,
                env,
                params,
            } => {
                let resp = cmd_launch(
                    LaunchContext {
                        home: home.as_ref(),
                        registry: &registry,
                        status_tx: &status_tx,
                        config: config.as_ref(),
                        cleanup_tx: cleanup_tx.clone(),
                    },
                    MissionLaunchConfig { wasm, env, params },
                    name,
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
            Command::Inspect { wasm } => {
                let wasm_path = match resolve_launch_wasm_path(&wasm) {
                    Ok(p) => p,
                    Err(message) => {
                        let _ = write_resp(&mut writer, &Response::Error { message }).await;
                        return;
                    }
                };
                let resp = match brrmmmm::controller::inspect_module_contract_async(&wasm_path)
                    .await
                {
                    Ok(inspection) => Response::Inspected { describe: inspection.describe },
                    Err(e) => Response::Error {
                        message: format!("inspect: {e:#}"),
                    },
                };
                let _ = write_resp(&mut writer, &resp).await;
            }
        }
    }
}

async fn cmd_launch(
    context: LaunchContext<'_>,
    launch: MissionLaunchConfig,
    name: Option<String>,
) -> Response {
    let LaunchContext {
        home,
        registry,
        status_tx,
        config,
        cleanup_tx,
    } = context;

    let wasm = match resolve_launch_wasm_path(&launch.wasm) {
        Ok(path) => path,
        Err(message) => {
            return Response::Error { message };
        }
    };

    let mut reg = registry.lock().await;

    if reg.missions.len() >= MAX_MISSIONS {
        return Response::Full {
            message: format!("daemon at {MAX_MISSIONS} mission capacity"),
        };
    }

    let mission_name = if let Some(name) = name {
        if reg.taken_names.contains(&name) {
            return Response::Error {
                message: format!("mission name '{name}' already taken"),
            };
        }
        name
    } else {
        match crate::names::generate_mission_name(&reg.taken_names) {
            Some(name) => name,
            None => {
                return Response::Error {
                    message: "could not generate a unique mission name".into(),
                };
            }
        }
    };

    if let Err(error) = prepare_mission_storage(home, &mission_name) {
        return Response::Error {
            message: format!("prepare mission storage: {error}"),
        };
    }

    let (event_tx, _) = broadcast::channel::<MissionEvent>(BROADCAST_CAPACITY);
    let (ctrl_tx, ctrl_rx) = mpsc::channel::<MissionCtrl>(32);
    let history = Arc::new(Mutex::new(MissionHistory::default()));
    let state = Arc::new(Mutex::new(MissionState {
        name: mission_name.clone(),
        wasm: wasm.clone(),
        state: MissionSchedulerState::Launching,
        held: false,
        terminal: false,
        phase: "idle".into(),
        cycles: 0,
        pid: None,
        last_started_at_ms: None,
        last_run_at_ms: None,
        last_outcome_status: None,
        next_wake_at_ms: None,
        last_error: None,
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

    publish_status_snapshot(registry, status_tx).await;

    tokio::spawn(mission_loop(MissionLoopArgs {
        home: home.to_path_buf(),
        name: mission_name.clone(),
        launch: MissionLaunchConfig {
            wasm,
            env: launch.env,
            params: launch.params,
        },
        state,
        history,
        event_tx,
        ctrl_rx,
        cleanup_tx,
        registry: Arc::clone(registry),
        status_tx: status_tx.clone(),
        config: Arc::new(config.clone()),
    }));

    Response::Launched {
        mission: mission_name,
    }
}

fn resolve_launch_wasm_path(wasm: &str) -> std::result::Result<String, String> {
    let path = Path::new(wasm);
    if !path.is_absolute() {
        return Err(format!(
            "daemon launch requires an absolute WASM path, got relative path '{wasm}'"
        ));
    }

    std::fs::canonicalize(path)
        .map_err(|error| format!("resolve mission module path '{wasm}': {error}"))
        .map(|path| path.to_string_lossy().into_owned())
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
                Err(broadcast::error::RecvError::Lagged(count)) => {
                    eprintln!("[brrmmmm daemon] watch '{name}' lagged by {count} events");
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

async fn cmd_watch_status(
    mut status_rx: watch::Receiver<Vec<MissionSummary>>,
    writer: &mut (impl AsyncWrite + Unpin),
) {
    let initial = status_rx.borrow().clone();
    if write_resp(writer, &Response::Status { missions: initial })
        .await
        .is_err()
    {
        return;
    }

    while status_rx.changed().await.is_ok() {
        let missions = status_rx.borrow().clone();
        if write_resp(writer, &Response::Status { missions })
            .await
            .is_err()
        {
            return;
        }
    }
}

async fn write_resp(writer: &mut (impl AsyncWrite + Unpin), resp: &Response) -> Result<()> {
    let json = serde_json::to_string(resp)?;
    writer.write_all(format!("{json}\n").as_bytes()).await?;
    Ok(())
}

async fn remove_mission(
    registry: &Arc<Mutex<Registry>>,
    status_tx: &watch::Sender<Vec<MissionSummary>>,
    name: &str,
) {
    let mut reg = registry.lock().await;
    reg.missions.remove(name);
    reg.taken_names.remove(name);
    drop(reg);
    publish_status_snapshot(registry, status_tx).await;
}

async fn publish_status_snapshot(
    registry: &Arc<Mutex<Registry>>,
    status_tx: &watch::Sender<Vec<MissionSummary>>,
) {
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
    summaries.sort_by(|left, right| left.name.cmp(&right.name));
    status_tx.send_replace(summaries);
}

async fn update_mission_state<F>(
    state: &Arc<Mutex<MissionState>>,
    registry: &Arc<Mutex<Registry>>,
    status_tx: &watch::Sender<Vec<MissionSummary>>,
    apply: F,
) where
    F: FnOnce(&mut MissionState),
{
    let mut locked = state.lock().await;
    apply(&mut locked);
    drop(locked);
    publish_status_snapshot(registry, status_tx).await;
}

async fn mission_loop(args: MissionLoopArgs) {
    let MissionLoopArgs {
        home,
        name,
        launch,
        state,
        history,
        event_tx,
        mut ctrl_rx,
        cleanup_tx,
        registry,
        status_tx,
        config,
    } = args;
    let _cleanup = MissionCleanup::new(name.clone(), cleanup_tx);
    let events_file = mission_events_file(&home, &name);
    let mut recorder = MissionRecorder::open(
        &events_file,
        Arc::clone(&state),
        history,
        event_tx,
        Arc::clone(&registry),
        status_tx.clone(),
    )
    .await;
    let mut mode = MissionLoopMode::Launch {
        override_retry_gate: false,
    };
    let mut held_resume_mode: Option<MissionLoopMode> = None;

    loop {
        if state.lock().await.held {
            match wait_while_held(
                &state,
                &registry,
                &status_tx,
                &mut ctrl_rx,
                held_resume_mode.take().unwrap_or(MissionLoopMode::Idle),
            )
            .await
            {
                MissionTransition::Continue(next_mode) => {
                    mode = next_mode;
                    continue;
                }
                MissionTransition::Abort => return,
                MissionTransition::Hold(_) => continue,
            }
        }

        match mode.clone() {
            MissionLoopMode::Launch {
                override_retry_gate,
            } => {
                set_loop_mode(&state, &registry, &status_tx, &mode).await;
                match run_mission_attempt(
                    &launch,
                    override_retry_gate,
                    AttemptContext {
                        mission_name: &name,
                        state: &state,
                        registry: &registry,
                        status_tx: &status_tx,
                        ctrl_rx: &mut ctrl_rx,
                        recorder: &mut recorder,
                        config: config.as_ref(),
                    },
                )
                .await
                {
                    MissionTransition::Continue(next_mode) => mode = next_mode,
                    MissionTransition::Hold(resume_mode) => {
                        held_resume_mode = Some(resume_mode);
                    }
                    MissionTransition::Abort => return,
                }
            }
            MissionLoopMode::Scheduled {
                wake_at_ms,
                override_retry_gate,
            } => match wait_for_schedule(
                &state,
                &registry,
                &status_tx,
                &mut ctrl_rx,
                wake_at_ms,
                override_retry_gate,
            )
            .await
            {
                MissionTransition::Continue(next_mode) => mode = next_mode,
                MissionTransition::Hold(resume_mode) => {
                    held_resume_mode = Some(resume_mode);
                }
                MissionTransition::Abort => return,
            },
            MissionLoopMode::Idle
            | MissionLoopMode::AwaitingChange
            | MissionLoopMode::AwaitingOperator
            | MissionLoopMode::TerminalFailure => {
                match wait_for_manual(&state, &registry, &status_tx, &mut ctrl_rx, mode.clone())
                    .await
                {
                    MissionTransition::Continue(next_mode) => mode = next_mode,
                    MissionTransition::Hold(resume_mode) => {
                        held_resume_mode = Some(resume_mode);
                    }
                    MissionTransition::Abort => return,
                }
            }
        }
    }
}

async fn wait_while_held(
    state: &Arc<Mutex<MissionState>>,
    registry: &Arc<Mutex<Registry>>,
    status_tx: &watch::Sender<Vec<MissionSummary>>,
    ctrl_rx: &mut mpsc::Receiver<MissionCtrl>,
    resume_mode: MissionLoopMode,
) -> MissionTransition {
    loop {
        match ctrl_rx.recv().await {
            Some(MissionCtrl::Resume) => {
                update_mission_state(state, registry, status_tx, |state| {
                    state.held = false;
                })
                .await;
                return MissionTransition::Continue(resume_mode);
            }
            Some(MissionCtrl::Abort) => return MissionTransition::Abort,
            Some(MissionCtrl::Retry) => {
                update_mission_state(state, registry, status_tx, |state| {
                    state.held = false;
                })
                .await;
                return MissionTransition::Continue(MissionLoopMode::Launch {
                    override_retry_gate: true,
                });
            }
            Some(MissionCtrl::Hold) => {}
            None => return MissionTransition::Abort,
        }
    }
}

async fn wait_for_schedule(
    state: &Arc<Mutex<MissionState>>,
    registry: &Arc<Mutex<Registry>>,
    status_tx: &watch::Sender<Vec<MissionSummary>>,
    ctrl_rx: &mut mpsc::Receiver<MissionCtrl>,
    wake_at_ms: u64,
    override_retry_gate: bool,
) -> MissionTransition {
    let scheduled_mode = MissionLoopMode::Scheduled {
        wake_at_ms,
        override_retry_gate,
    };
    set_loop_mode(state, registry, status_tx, &scheduled_mode).await;

    loop {
        let now = now_ms();
        if now >= wake_at_ms {
            return MissionTransition::Continue(MissionLoopMode::Launch {
                override_retry_gate,
            });
        }

        let remaining_ms = wake_at_ms.saturating_sub(now).min(1_000);
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(remaining_ms)) => {}
            ctrl = ctrl_rx.recv() => {
                match ctrl {
                    Some(MissionCtrl::Hold) => {
                        set_held_state(state, registry, status_tx).await;
                        return MissionTransition::Hold(MissionLoopMode::Launch {
                            override_retry_gate,
                        });
                    }
                    Some(MissionCtrl::Retry) => {
                        return MissionTransition::Continue(MissionLoopMode::Launch {
                            override_retry_gate: true,
                        });
                    }
                    Some(MissionCtrl::Abort) => return MissionTransition::Abort,
                    Some(MissionCtrl::Resume) => {}
                    None => return MissionTransition::Abort,
                }
            }
        }
    }
}

async fn wait_for_manual(
    state: &Arc<Mutex<MissionState>>,
    registry: &Arc<Mutex<Registry>>,
    status_tx: &watch::Sender<Vec<MissionSummary>>,
    ctrl_rx: &mut mpsc::Receiver<MissionCtrl>,
    mode: MissionLoopMode,
) -> MissionTransition {
    set_loop_mode(state, registry, status_tx, &mode).await;

    loop {
        match ctrl_rx.recv().await {
            Some(MissionCtrl::Hold) => {
                set_held_state(state, registry, status_tx).await;
                return MissionTransition::Hold(mode.clone());
            }
            Some(MissionCtrl::Retry) => {
                return MissionTransition::Continue(MissionLoopMode::Launch {
                    override_retry_gate: true,
                });
            }
            Some(MissionCtrl::Abort) => return MissionTransition::Abort,
            Some(MissionCtrl::Resume) => {}
            None => return MissionTransition::Abort,
        }
    }
}

async fn run_mission_attempt(
    launch: &MissionLaunchConfig,
    override_retry_gate: bool,
    context: AttemptContext<'_>,
) -> MissionTransition {
    let AttemptContext {
        mission_name,
        state,
        registry,
        status_tx,
        ctrl_rx,
        recorder,
        config,
    } = context;

    recorder
        .record(Event::EnvSnapshot {
            ts: now_ts(),
            vars: EnvVarStatus::from_raw_env(&launch_env_pairs(&launch.env)),
        })
        .await;

    let (runtime_tx, mut runtime_rx) = mpsc::unbounded_channel::<Event>();
    let event_sink = EventSink::for_callback(move |event| {
        let _ = runtime_tx.send(event);
    });
    let controller = MissionController::new(
        &launch.wasm,
        launch_env_pairs(&launch.env),
        launch.params.clone().map(String::into_bytes),
        false,
        override_retry_gate,
        event_sink,
        config,
    );

    let mut controller: Option<MissionController> = match controller {
        Ok(controller) => Some(controller),
        Err(error) => {
            let message = format!("failed to start mission '{mission_name}': {error:#}");
            update_mission_state(state, registry, status_tx, |state| {
                state.last_error = Some(message.clone());
            })
            .await;
            recorder.log(message).await;
            return MissionTransition::Continue(MissionLoopMode::TerminalFailure);
        }
    };

    loop {
        drain_runtime_events(&mut runtime_rx, recorder).await;

        if let Some(active) = controller.as_ref()
            && let Some(completion) = active.poll_completion()
        {
            if let Some(active) = controller.take() {
                active.stop();
            }
            flush_runtime_events(&mut runtime_rx, recorder, Duration::from_millis(250)).await;
            let next_mode = next_mode_after_completion(&completion, config);
            set_loop_mode(state, registry, status_tx, &next_mode).await;
            return MissionTransition::Continue(next_mode);
        }

        tokio::select! {
            ctrl = ctrl_rx.recv() => {
                match ctrl {
                    Some(MissionCtrl::Hold) => {
                        if let Some(active) = controller.take() {
                            active.stop();
                        }
                        flush_runtime_events(&mut runtime_rx, recorder, Duration::from_millis(250)).await;
                        set_held_state(state, registry, status_tx).await;
                        return MissionTransition::Hold(MissionLoopMode::Launch {
                            override_retry_gate: false,
                        });
                    }
                    Some(MissionCtrl::Retry) => {
                        if let Some(active) = controller.take() {
                            active.stop();
                        }
                        flush_runtime_events(&mut runtime_rx, recorder, Duration::from_millis(250)).await;
                        return MissionTransition::Continue(MissionLoopMode::Launch {
                            override_retry_gate: true,
                        });
                    }
                    Some(MissionCtrl::Abort) => {
                        if let Some(active) = controller.take() {
                            active.stop();
                        }
                        flush_runtime_events(&mut runtime_rx, recorder, Duration::from_millis(250)).await;
                        return MissionTransition::Abort;
                    }
                    Some(MissionCtrl::Resume) => {}
                    None => {
                        if let Some(active) = controller.take() {
                            active.stop();
                        }
                        flush_runtime_events(&mut runtime_rx, recorder, Duration::from_millis(250)).await;
                        return MissionTransition::Abort;
                    }
                }
            }
            maybe_event = runtime_rx.recv() => {
                if let Some(event) = maybe_event {
                    recorder.record(event).await;
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
    }
}

async fn drain_runtime_events(
    runtime_rx: &mut mpsc::UnboundedReceiver<Event>,
    recorder: &mut MissionRecorder,
) {
    while let Ok(event) = runtime_rx.try_recv() {
        recorder.record(event).await;
    }
}

async fn flush_runtime_events(
    runtime_rx: &mut mpsc::UnboundedReceiver<Event>,
    recorder: &mut MissionRecorder,
    window: Duration,
) {
    let deadline = Instant::now() + window;
    loop {
        drain_runtime_events(runtime_rx, recorder).await;

        if Instant::now() >= deadline {
            return;
        }

        let remaining = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(remaining, runtime_rx.recv()).await {
            Ok(Some(event)) => recorder.record(event).await,
            Ok(None) | Err(_) => return,
        }
    }
}

async fn set_loop_mode(
    state: &Arc<Mutex<MissionState>>,
    registry: &Arc<Mutex<Registry>>,
    status_tx: &watch::Sender<Vec<MissionSummary>>,
    mode: &MissionLoopMode,
) {
    update_mission_state(state, registry, status_tx, |state| match mode {
        MissionLoopMode::Launch { .. } => {
            state.state = MissionSchedulerState::Launching;
            state.held = false;
            state.terminal = false;
            state.next_wake_at_ms = None;
        }
        MissionLoopMode::Scheduled { wake_at_ms, .. } => {
            state.state = MissionSchedulerState::Scheduled;
            state.held = false;
            state.terminal = false;
            state.next_wake_at_ms = Some(*wake_at_ms);
        }
        MissionLoopMode::Idle => {
            state.state = MissionSchedulerState::Idle;
            state.held = false;
            state.terminal = false;
            state.next_wake_at_ms = None;
        }
        MissionLoopMode::AwaitingChange => {
            state.state = MissionSchedulerState::AwaitingChange;
            state.held = false;
            state.terminal = true;
            state.next_wake_at_ms = None;
        }
        MissionLoopMode::AwaitingOperator => {
            state.state = MissionSchedulerState::AwaitingOperator;
            state.held = false;
            state.terminal = true;
            state.next_wake_at_ms = None;
        }
        MissionLoopMode::TerminalFailure => {
            state.state = MissionSchedulerState::TerminalFailure;
            state.held = false;
            state.terminal = true;
            state.next_wake_at_ms = None;
        }
    })
    .await;
}

async fn set_held_state(
    state: &Arc<Mutex<MissionState>>,
    registry: &Arc<Mutex<Registry>>,
    status_tx: &watch::Sender<Vec<MissionSummary>>,
) {
    update_mission_state(state, registry, status_tx, |state| {
        state.state = MissionSchedulerState::Held;
        state.held = true;
        state.terminal = false;
        state.next_wake_at_ms = None;
    })
    .await;
}

fn apply_runtime_event(state: &mut MissionState, event: &Event) {
    match event {
        Event::Started { .. } => {
            state.state = MissionSchedulerState::Running;
            state.last_started_at_ms = Some(now_ms());
            state.next_wake_at_ms = None;
            state.last_error = None;
        }
        Event::Phase { phase, .. } => {
            state.phase = phase_name(phase).to_string();
        }
        Event::RequestError { message, .. } => {
            state.last_error = Some(message.clone());
        }
        Event::ArtifactReceived { kind, .. } if kind == "published_output" => {
            state.last_error = None;
        }
        Event::MissionOutcome { outcome, .. } => {
            state.cycles = state.cycles.saturating_add(1);
            state.last_run_at_ms = Some(now_ms());
            state.last_outcome_status = Some(outcome_status_name(outcome.status).to_string());
            if outcome.status == MissionOutcomeStatus::Published {
                state.last_error = None;
            } else {
                state.last_error = Some(outcome.message.clone());
            }
            state.next_wake_at_ms = None;
        }
        Event::ModuleExit { .. } => {
            state.pid = None;
        }
        _ => {}
    }
}

fn next_mode_after_completion(completion: &MissionCompletion, config: &Config) -> MissionLoopMode {
    match completion.outcome.status {
        MissionOutcomeStatus::Published => completion
            .snapshot
            .describe
            .as_ref()
            .and_then(|describe| describe.poll_strategy.as_ref())
            .map_or(MissionLoopMode::Idle, scheduled_mode_from_strategy),
        MissionOutcomeStatus::RetryableFailure => {
            if matches!(
                completion
                    .snapshot
                    .last_host_decision
                    .as_ref()
                    .map(|decision| decision.next_attempt_policy),
                Some(NextAttemptPolicy::ManualOnly)
            ) {
                return MissionLoopMode::AwaitingChange;
            }

            completion
                .snapshot
                .cooldown_until_ms
                .or(completion.snapshot.next_allowed_at_ms)
                .map_or_else(
                    || {
                        completion
                            .snapshot
                            .describe
                            .as_ref()
                            .and_then(|describe| describe.poll_strategy.as_ref())
                            .map_or_else(
                                || MissionLoopMode::Scheduled {
                                    wake_at_ms: now_ms()
                                        .saturating_add(config.assurance.default_retry_after_ms),
                                    override_retry_gate: false,
                                },
                                |strategy| {
                                    scheduled_mode_with_backoff(
                                        strategy,
                                        completion.snapshot.consecutive_failures,
                                    )
                                },
                            )
                    },
                    |wake_at_ms| MissionLoopMode::Scheduled {
                        wake_at_ms,
                        override_retry_gate: false,
                    },
                )
        }
        MissionOutcomeStatus::OperatorActionRequired => MissionLoopMode::AwaitingOperator,
        MissionOutcomeStatus::TerminalFailure => MissionLoopMode::TerminalFailure,
    }
}

fn scheduled_mode_from_strategy(strategy: &PollStrategy) -> MissionLoopMode {
    scheduled_mode_with_backoff(strategy, 0)
}

fn scheduled_mode_with_backoff(
    strategy: &PollStrategy,
    consecutive_failures: u32,
) -> MissionLoopMode {
    MissionLoopMode::Scheduled {
        wake_at_ms: now_ms().saturating_add(strategy_backoff_ms(strategy, consecutive_failures)),
        override_retry_gate: false,
    }
}

fn strategy_backoff_ms(strategy: &PollStrategy, consecutive_failures: u32) -> u64 {
    match strategy {
        PollStrategy::FixedInterval { interval_secs } => u64::from(*interval_secs) * 1_000,
        PollStrategy::ExponentialBackoff {
            base_secs,
            max_secs,
        } => {
            let shift = consecutive_failures.saturating_sub(1);
            let factor = 1u64.checked_shl(shift).unwrap_or(u64::MAX);
            u64::from(*base_secs)
                .saturating_mul(factor)
                .min(u64::from(*max_secs))
                * 1_000
        }
        PollStrategy::Jittered {
            base_secs,
            jitter_secs,
        } => u64::from((*base_secs).saturating_sub(*jitter_secs)) * 1_000,
    }
}

fn launch_env_pairs(env: &HashMap<String, String>) -> Vec<(String, String)> {
    env.iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn phase_name(phase: &MissionPhase) -> &'static str {
    match phase {
        MissionPhase::Idle => "idle",
        MissionPhase::CoolingDown => "cooling_down",
        MissionPhase::Fetching => "fetching",
        MissionPhase::Parsing => "parsing",
        MissionPhase::Publishing => "publishing",
        MissionPhase::Failed => "failed",
    }
}

const fn outcome_status_name(status: MissionOutcomeStatus) -> &'static str {
    match status {
        MissionOutcomeStatus::Published => "published",
        MissionOutcomeStatus::RetryableFailure => "retryable_failure",
        MissionOutcomeStatus::TerminalFailure => "terminal_failure",
        MissionOutcomeStatus::OperatorActionRequired => "operator_action_required",
    }
}

#[cfg(test)]
mod tests;
