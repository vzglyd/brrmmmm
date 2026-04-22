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

use brrmmmm::abi::{
    HostDecisionState, MissionModuleDescribe, MissionOutcome, MissionOutcomeStatus, MissionPhase,
    NextAttemptPolicy, OperatorEscalationState, PollStrategy,
};
use brrmmmm::config::Config;
use brrmmmm::controller::{MissionCompletion, MissionController};
use brrmmmm::events::{EnvVarStatus, Event, EventSink, ms_to_iso8601, now_ms, now_ts};

use mission::{
    BROADCAST_CAPACITY, MAX_MISSIONS, MissionCleanup, MissionCtrl, MissionEvent, MissionHandle,
    MissionHistory, MissionState,
};
use protocol::{Command, MissionSchedulerState, MissionSummary, Response};
use crate::mission_result::{
    ExplanationRecord, MissionAttemptRecord, MissionChallengeRecord, MissionInterventionRecord,
    MissionJobFilesRecord, MissionJobMode, MissionJobRecord, MissionModuleRecord,
    MissionRecordContext, MissionRecorder as ResultFileRecorder, MissionStatsRecord,
    MissionStatusRecord, MissionTimelineEntry, escalation_record, host_decision_record,
    mission_contract_record, write_status_record,
};

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

fn mission_status_file(home: &Path, mission: &str) -> PathBuf {
    mission_dir(home, mission).join(format!("{mission}.status.json"))
}

fn mission_result_file(home: &Path, mission: &str) -> PathBuf {
    mission_dir(home, mission).join(format!("{mission}.out.json"))
}

fn prepare_mission_storage(home: &Path, mission: &str) -> Result<()> {
    let dir = mission_dir(home, mission);
    std::fs::create_dir_all(&dir)?;

    let events_file = mission_events_file(home, mission);
    if events_file.exists() {
        std::fs::remove_file(&events_file)?;
    }

    let legacy_result_file = mission_dir(home, mission).join(format!("{mission}.out"));
    if legacy_result_file.exists() {
        std::fs::remove_file(&legacy_result_file)?;
    }

    let status_file = mission_status_file(home, mission);
    if status_file.exists() {
        std::fs::remove_file(&status_file)?;
    }

    let result_file = mission_result_file(home, mission);
    if result_file.exists() {
        std::fs::remove_file(&result_file)?;
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

const MAX_TIMELINE_ENTRIES: usize = 128;
const MAX_CHALLENGE_ENTRIES: usize = 32;
const MAX_INTERVENTION_ENTRIES: usize = 32;

struct MissionRecorder {
    file: Option<tokio::fs::File>,
    status_path: PathBuf,
    result_path: PathBuf,
    mission_name: String,
    wasm_path: String,
    state: Arc<Mutex<MissionState>>,
    history: Arc<Mutex<MissionHistory>>,
    event_tx: broadcast::Sender<MissionEvent>,
    registry: Arc<Mutex<Registry>>,
    status_tx: watch::Sender<Vec<MissionSummary>>,
    current_scheduler_state: Option<MissionSchedulerState>,
    attempt_sequence: u64,
    current_attempt_started_at_ms: Option<u64>,
    current_attempt_started_at: Option<String>,
    last_update_at_ms: u64,
    describe: Option<MissionModuleDescribe>,
    outcome: Option<MissionOutcome>,
    host_decision: Option<HostDecisionState>,
    escalation: Option<OperatorEscalationState>,
    stats: MissionStatsRecord,
    timeline: Vec<MissionTimelineEntry>,
    challenges: Vec<MissionChallengeRecord>,
    interventions: Vec<MissionInterventionRecord>,
}

struct AttemptContext<'a> {
    home: &'a Path,
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
        status_path: PathBuf,
        result_path: PathBuf,
        mission_name: String,
        wasm_path: String,
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

        let mut recorder = Self {
            file,
            status_path,
            result_path,
            mission_name,
            wasm_path,
            state,
            history,
            event_tx,
            registry,
            status_tx,
            current_scheduler_state: None,
            attempt_sequence: 0,
            current_attempt_started_at_ms: None,
            current_attempt_started_at: None,
            last_update_at_ms: now_ms(),
            describe: None,
            outcome: None,
            host_decision: None,
            escalation: None,
            stats: MissionStatsRecord::default(),
            timeline: Vec::new(),
            challenges: Vec::new(),
            interventions: Vec::new(),
        };
        recorder.write_status_snapshot().await;
        recorder
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

        self.capture_event(&event);

        let history_event = self.history.lock().await.append(line);
        let _ = self.event_tx.send(history_event);

        if status_worthy_event(&event) {
            self.write_status_snapshot().await;
        }
    }

    async fn log(&mut self, message: impl Into<String>) {
        self.record(Event::Log {
            ts: now_ts(),
            message: message.into(),
        })
        .await;
    }

    async fn record_scheduler_state(&mut self, state: MissionSchedulerState) {
        if self.current_scheduler_state == Some(state) {
            return;
        }
        let ts_ms = now_ms();
        let ts = ms_to_iso8601(ts_ms);
        if state == MissionSchedulerState::Launching {
            self.begin_attempt(ts_ms, ts.clone());
        }
        self.current_scheduler_state = Some(state);
        self.record(Event::SchedulerState {
            ts,
            state: scheduler_state_name(state).to_string(),
        })
        .await;
    }

    async fn record_intervention(&mut self, actor: String, action: String, reason: Option<String>) {
        self.record(Event::Intervention {
            ts: now_ts(),
            actor,
            action,
            reason,
        })
        .await;
    }

    fn set_completion_stats(&mut self, completion: &MissionCompletion) {
        self.describe = completion.snapshot.describe.clone();
        self.outcome = Some(completion.outcome.clone());
        self.host_decision = completion.snapshot.last_host_decision.clone();
        self.escalation = completion.snapshot.pending_operator_action.clone();
        self.stats = MissionStatsRecord {
            consecutive_failures: completion.snapshot.consecutive_failures,
            last_success_at_ms: completion.snapshot.last_success_at_ms,
            last_failure_at_ms: completion.snapshot.last_failure_at_ms,
            cooldown_until_ms: completion.snapshot.cooldown_until_ms,
        };
    }

    fn build_result_context(&self, scheduler_state: MissionSchedulerState) -> MissionRecordContext {
        let (held, cycles) = self
            .state
            .try_lock()
            .map(|state| (state.held, state.cycles))
            .unwrap_or((false, 0));
        MissionRecordContext {
            job: Some(self.job_record(scheduler_state, held, cycles)),
            mission: Some(mission_contract_record(
                Some(&self.wasm_path),
                self.describe.as_ref(),
            )),
            attempt: self.attempt_record(Some(scheduler_state), true),
            timeline: self.timeline.clone(),
            challenges: self.challenges.clone(),
            interventions: self.interventions.clone(),
        }
    }

    fn begin_attempt(&mut self, started_at_ms: u64, started_at: String) {
        self.attempt_sequence = self.attempt_sequence.saturating_add(1);
        self.current_attempt_started_at_ms = Some(started_at_ms);
        self.current_attempt_started_at = Some(started_at);
        self.last_update_at_ms = started_at_ms;
        self.describe = None;
        self.outcome = None;
        self.host_decision = None;
        self.escalation = None;
        self.timeline.clear();
        self.challenges.clear();
        self.interventions.clear();
    }

    fn capture_event(&mut self, event: &Event) {
        self.last_update_at_ms = now_ms();
        match event {
            Event::Describe { describe, .. } => {
                self.describe = Some(describe.clone());
            }
            Event::Started { ts, .. } => {
                self.push_timeline(MissionTimelineEntry {
                    ts: ts.clone(),
                    ts_ms: self.last_update_at_ms,
                    kind: "runtime_started".to_string(),
                    summary: "Mission runtime started.".to_string(),
                    detail: None,
                    scheduler_state: self
                        .current_scheduler_state
                        .map(|state| scheduler_state_name(state).to_string()),
                    phase: None,
                    outcome_status: None,
                });
            }
            Event::Phase { ts, phase } => {
                self.push_timeline(MissionTimelineEntry {
                    ts: ts.clone(),
                    ts_ms: self.last_update_at_ms,
                    kind: "phase".to_string(),
                    summary: format!("Phase changed to {}.", phase_name(phase)),
                    detail: None,
                    scheduler_state: self
                        .current_scheduler_state
                        .map(|state| scheduler_state_name(state).to_string()),
                    phase: Some(phase_name(phase).to_string()),
                    outcome_status: None,
                });
            }
            Event::SchedulerState { ts, state } => {
                self.push_timeline(MissionTimelineEntry {
                    ts: ts.clone(),
                    ts_ms: self.last_update_at_ms,
                    kind: "scheduler_state".to_string(),
                    summary: format!("Scheduler entered {state}."),
                    detail: None,
                    scheduler_state: Some(state.clone()),
                    phase: None,
                    outcome_status: None,
                });
            }
            Event::ArtifactReceived {
                ts,
                kind,
                size_bytes,
                ..
            } if kind == "published_output" => {
                self.push_timeline(MissionTimelineEntry {
                    ts: ts.clone(),
                    ts_ms: self.last_update_at_ms,
                    kind: "artifact".to_string(),
                    summary: format!("Published output received ({size_bytes} bytes)."),
                    detail: None,
                    scheduler_state: self
                        .current_scheduler_state
                        .map(|state| scheduler_state_name(state).to_string()),
                    phase: Some("publishing".to_string()),
                    outcome_status: None,
                });
            }
            Event::RequestError {
                ts,
                error_kind,
                message,
                ..
            } => {
                self.push_challenge(MissionChallengeRecord {
                    ts: ts.clone(),
                    ts_ms: self.last_update_at_ms,
                    kind: error_kind.clone(),
                    summary: "Network request failed.".to_string(),
                    detail: Some(message.clone()),
                });
            }
            Event::BrowserActionDone {
                ts,
                action,
                error: Some(error),
                ok: false,
                ..
            } => {
                self.push_challenge(MissionChallengeRecord {
                    ts: ts.clone(),
                    ts_ms: self.last_update_at_ms,
                    kind: "browser_action_failure".to_string(),
                    summary: format!("Browser action {action} failed."),
                    detail: Some(error.clone()),
                });
            }
            Event::AiRequestDone {
                ts,
                action,
                error: Some(error),
                ok: false,
                ..
            } => {
                self.push_challenge(MissionChallengeRecord {
                    ts: ts.clone(),
                    ts_ms: self.last_update_at_ms,
                    kind: "ai_request_failure".to_string(),
                    summary: format!("AI request {action} failed."),
                    detail: Some(error.clone()),
                });
            }
            Event::MissionOutcome {
                ts,
                outcome,
                host_decision,
                escalation,
                ..
            } => {
                self.outcome = Some(outcome.clone());
                self.host_decision = Some(host_decision.clone());
                self.escalation = escalation.clone();
                self.push_timeline(MissionTimelineEntry {
                    ts: ts.clone(),
                    ts_ms: self.last_update_at_ms,
                    kind: "outcome".to_string(),
                    summary: format!(
                        "Mission closed as {}.",
                        outcome_status_name(outcome.status)
                    ),
                    detail: Some(outcome.message.clone()),
                    scheduler_state: self
                        .current_scheduler_state
                        .map(|state| scheduler_state_name(state).to_string()),
                    phase: None,
                    outcome_status: Some(outcome_status_name(outcome.status).to_string()),
                });
                if outcome.status != MissionOutcomeStatus::Published {
                    self.push_challenge(MissionChallengeRecord {
                        ts: ts.clone(),
                        ts_ms: self.last_update_at_ms,
                        kind: outcome.reason_code.clone(),
                        summary: format!(
                            "Mission reported {}.",
                            outcome_status_name(outcome.status)
                        ),
                        detail: Some(outcome.message.clone()),
                    });
                }
            }
            Event::Intervention {
                ts,
                actor,
                action,
                reason,
            } => {
                self.push_intervention(MissionInterventionRecord {
                    ts: ts.clone(),
                    ts_ms: self.last_update_at_ms,
                    actor: actor.clone(),
                    action: action.clone(),
                    reason: reason.clone(),
                });
            }
            _ => {}
        }
    }

    fn push_timeline(&mut self, entry: MissionTimelineEntry) {
        push_capped(&mut self.timeline, entry, MAX_TIMELINE_ENTRIES);
    }

    fn push_challenge(&mut self, challenge: MissionChallengeRecord) {
        self.push_timeline(MissionTimelineEntry {
            ts: challenge.ts.clone(),
            ts_ms: challenge.ts_ms,
            kind: "challenge".to_string(),
            summary: challenge.summary.clone(),
            detail: challenge.detail.clone(),
            scheduler_state: self
                .current_scheduler_state
                .map(|state| scheduler_state_name(state).to_string()),
            phase: None,
            outcome_status: None,
        });
        push_capped(&mut self.challenges, challenge, MAX_CHALLENGE_ENTRIES);
    }

    fn push_intervention(&mut self, intervention: MissionInterventionRecord) {
        self.push_timeline(MissionTimelineEntry {
            ts: intervention.ts.clone(),
            ts_ms: intervention.ts_ms,
            kind: "intervention".to_string(),
            summary: format!("{} performed {}.", intervention.actor, intervention.action),
            detail: intervention.reason.clone(),
            scheduler_state: self
                .current_scheduler_state
                .map(|state| scheduler_state_name(state).to_string()),
            phase: None,
            outcome_status: None,
        });
        push_capped(
            &mut self.interventions,
            intervention,
            MAX_INTERVENTION_ENTRIES,
        );
    }

    fn attempt_record(
        &self,
        scheduler_state: Option<MissionSchedulerState>,
        terminal: bool,
    ) -> Option<MissionAttemptRecord> {
        let started_at = self.current_attempt_started_at.clone()?;
        let updated_at = ms_to_iso8601(self.last_update_at_ms);
        Some(MissionAttemptRecord {
            sequence: self.attempt_sequence,
            scheduler_state: scheduler_state
                .or(self.current_scheduler_state)
                .map(|state| scheduler_state_name(state).to_string()),
            phase: self
                .state
                .try_lock()
                .ok()
                .map(|state| state.phase.clone()),
            started_at,
            updated_at: updated_at.clone(),
            finished_at: terminal.then_some(updated_at),
            terminal,
        })
    }

    fn job_record(
        &self,
        scheduler_state: MissionSchedulerState,
        held: bool,
        cycles: u64,
    ) -> MissionJobRecord {
        MissionJobRecord {
            mode: MissionJobMode::Daemon,
            name: Some(self.mission_name.clone()),
            scheduler_state: Some(scheduler_state_name(scheduler_state).to_string()),
            held,
            cycles,
            files: MissionJobFilesRecord {
                result_path: self.result_path.display().to_string(),
                status_path: Some(self.status_path.display().to_string()),
                events_path: Some(self.mission_events_path()),
            },
        }
    }

    fn mission_events_path(&self) -> String {
        self.status_path
            .parent()
            .map(|parent| parent.join("events.ndjson"))
            .unwrap_or_else(|| PathBuf::from("events.ndjson"))
            .display()
            .to_string()
    }

    async fn write_status_snapshot(&mut self) {
        let state = self.state.lock().await;
        let scheduler_state = state.state;
        let held = state.held;
        let cycles = state.cycles;
        let last_error = state.last_error.clone();
        drop(state);

        let host_decision = self
            .outcome
            .as_ref()
            .zip(self.host_decision.clone())
            .map(|(outcome, decision)| host_decision_record(decision, outcome, false));
        let escalation = self.escalation.as_ref().map(escalation_record);
        let explanation = self.outcome.as_ref().zip(host_decision.as_ref()).map(
            |(outcome, _host_decision)| ExplanationRecord {
                summary: format!(
                    "Live daemon snapshot for {}.",
                    self.mission_name
                ),
                message: outcome.message.clone(),
                next_action: match &escalation {
                    Some(escalation) => format!(
                        "{} Rescue window closes at {}.",
                        escalation.action, escalation.deadline_at
                    ),
                    None => outcome.message.clone(),
                },
            },
        );
        let record = MissionStatusRecord {
            schema_version: 1,
            record_kind: crate::mission_result::MissionRecordKind::Status,
            job: self.job_record(scheduler_state, held, cycles),
            mission: mission_contract_record(Some(&self.wasm_path), self.describe.as_ref()),
            module: MissionModuleRecord {
                wasm_path: Some(self.wasm_path.clone()),
                logical_id: self.describe.as_ref().map(|describe| describe.logical_id.clone()),
                name: self.describe.as_ref().map(|describe| describe.name.clone()),
                abi_version: self
                    .describe
                    .as_ref()
                    .map(|describe| describe.abi_version)
                    .filter(|abi_version| *abi_version != 0),
            },
            attempt: self.attempt_record(Some(scheduler_state), self.outcome.is_some()),
            timeline: self.timeline.clone(),
            challenges: self.challenges.clone(),
            interventions: self.interventions.clone(),
            outcome: self.outcome.clone(),
            host_decision,
            explanation,
            escalation,
            last_error,
            stats: self.stats.clone(),
            updated_at: if self.outcome.is_some() {
                ms_to_iso8601(self.last_update_at_ms)
            } else if let Some(started_at_ms) = self.current_attempt_started_at_ms {
                ms_to_iso8601(self.last_update_at_ms.max(started_at_ms))
            } else {
                ms_to_iso8601(now_ms())
            },
        };

        if let Err(error) = write_status_record(&self.status_path, &record) {
            eprintln!(
                "[brrmmmm daemon] failed to write status file {}: {error:#}",
                self.status_path.display()
            );
        }
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
        if ctrl_tx
            .send(MissionCtrl::Abort {
                actor: "daemon".to_string(),
                action: "abort".to_string(),
                reason: "daemon shutdown".to_string(),
            })
            .await
            .is_err()
        {
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
            Command::Hold { mission, reason } => {
                let resp = send_ctrl(&registry, &mission, MissionCtrl::Hold { reason }).await;
                let _ = write_resp(&mut writer, &resp).await;
            }
            Command::Resume { mission } => {
                let resp = send_ctrl(&registry, &mission, MissionCtrl::Resume).await;
                let _ = write_resp(&mut writer, &resp).await;
            }
            Command::Abort { mission, reason } => {
                let resp = send_ctrl(
                    &registry,
                    &mission,
                    MissionCtrl::Abort {
                        actor: "operator".to_string(),
                        action: "abort".to_string(),
                        reason,
                    },
                )
                .await;
                let _ = write_resp(&mut writer, &resp).await;
            }
            Command::Rescue {
                mission,
                action,
                reason,
            } => {
                let resp = send_ctrl(
                    &registry,
                    &mission,
                    rescue_control(action, reason),
                )
                .await;
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

fn rescue_control(action: RescueAction, reason: String) -> MissionCtrl {
    match action {
        RescueAction::Retry => MissionCtrl::Retry {
            actor: "operator".to_string(),
            action: "rescue_retry".to_string(),
            reason,
        },
        RescueAction::Abort => MissionCtrl::Abort {
            actor: "operator".to_string(),
            action: "rescue_abort".to_string(),
            reason,
        },
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
    let status_file = mission_status_file(&home, &name);
    let result_file = mission_result_file(&home, &name);
    let mut recorder = MissionRecorder::open(
        &events_file,
        status_file,
        result_file,
        name.clone(),
        launch.wasm.clone(),
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
                &mut recorder,
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
                set_loop_mode(&state, &registry, &status_tx, &mode, &mut recorder).await;
                match run_mission_attempt(
                    &launch,
                    override_retry_gate,
                    AttemptContext {
                        home: &home,
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
                &mut recorder,
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
                match wait_for_manual(
                    &state,
                    &registry,
                    &status_tx,
                    &mut ctrl_rx,
                    mode.clone(),
                    &mut recorder,
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
        }
    }
}

async fn wait_while_held(
    state: &Arc<Mutex<MissionState>>,
    registry: &Arc<Mutex<Registry>>,
    status_tx: &watch::Sender<Vec<MissionSummary>>,
    ctrl_rx: &mut mpsc::Receiver<MissionCtrl>,
    resume_mode: MissionLoopMode,
    recorder: &mut MissionRecorder,
) -> MissionTransition {
    loop {
        match ctrl_rx.recv().await {
            Some(MissionCtrl::Resume) => {
                update_mission_state(state, registry, status_tx, |state| {
                    state.held = false;
                })
                .await;
                recorder
                    .record_intervention("operator".to_string(), "resume".to_string(), None)
                    .await;
                return MissionTransition::Continue(resume_mode);
            }
            Some(MissionCtrl::Abort {
                actor,
                action,
                reason,
            }) => {
                recorder.record_intervention(actor, action, Some(reason)).await;
                return MissionTransition::Abort;
            }
            Some(MissionCtrl::Retry {
                actor,
                action,
                reason,
            }) => {
                update_mission_state(state, registry, status_tx, |state| {
                    state.held = false;
                })
                .await;
                recorder
                    .record_intervention(actor, action, Some(reason))
                    .await;
                return MissionTransition::Continue(MissionLoopMode::Launch {
                    override_retry_gate: true,
                });
            }
            Some(MissionCtrl::Hold { .. }) => {}
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
    recorder: &mut MissionRecorder,
) -> MissionTransition {
    let scheduled_mode = MissionLoopMode::Scheduled {
        wake_at_ms,
        override_retry_gate,
    };
    set_loop_mode(state, registry, status_tx, &scheduled_mode, recorder).await;

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
                    Some(MissionCtrl::Hold { reason }) => {
                        recorder
                            .record_intervention("operator".to_string(), "hold".to_string(), Some(reason))
                            .await;
                        set_held_state(state, registry, status_tx, recorder).await;
                        return MissionTransition::Hold(MissionLoopMode::Launch {
                            override_retry_gate,
                        });
                    }
                    Some(MissionCtrl::Retry { actor, action, reason }) => {
                        recorder.record_intervention(actor, action, Some(reason)).await;
                        return MissionTransition::Continue(MissionLoopMode::Launch {
                            override_retry_gate: true,
                        });
                    }
                    Some(MissionCtrl::Abort { actor, action, reason }) => {
                        recorder.record_intervention(actor, action, Some(reason)).await;
                        return MissionTransition::Abort;
                    }
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
    recorder: &mut MissionRecorder,
) -> MissionTransition {
    set_loop_mode(state, registry, status_tx, &mode, recorder).await;

    loop {
        match ctrl_rx.recv().await {
            Some(MissionCtrl::Hold { reason }) => {
                recorder
                    .record_intervention("operator".to_string(), "hold".to_string(), Some(reason))
                    .await;
                set_held_state(state, registry, status_tx, recorder).await;
                return MissionTransition::Hold(mode.clone());
            }
            Some(MissionCtrl::Retry { actor, action, reason }) => {
                recorder.record_intervention(actor, action, Some(reason)).await;
                return MissionTransition::Continue(MissionLoopMode::Launch {
                    override_retry_gate: true,
                });
            }
            Some(MissionCtrl::Abort { actor, action, reason }) => {
                recorder.record_intervention(actor, action, Some(reason)).await;
                return MissionTransition::Abort;
            }
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
        home,
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

    let result_path = mission_result_file(home, mission_name);
    let result_recorder =
        ResultFileRecorder::for_daemon(result_path.clone(), Some(Path::new(&launch.wasm)));

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
            let persisted = result_recorder.write_runtime_error_with_context(
                &error,
                recorder.build_result_context(MissionSchedulerState::TerminalFailure),
            );
            let message = match persisted {
                Ok(()) => message,
                Err(write_error) => format!(
                    "{message}; failed to write result file {}: {write_error:#}",
                    result_path.display()
                ),
            };
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
            recorder.set_completion_stats(&completion);
            let next_mode = next_mode_after_completion(&completion, config);
            let next_scheduler_state = loop_mode_state(&next_mode);
            if let Err(error) = result_recorder.write_completion_with_context(
                &completion,
                recorder.build_result_context(next_scheduler_state),
            ) {
                let message = format!(
                    "mission outcome recorded but result file {} could not be written: {error:#}",
                    result_path.display()
                );
                update_mission_state(state, registry, status_tx, |state| {
                    state.last_error = Some(message.clone());
                })
                .await;
                recorder.log(message).await;
                return MissionTransition::Continue(MissionLoopMode::TerminalFailure);
            }
            set_loop_mode(state, registry, status_tx, &next_mode, recorder).await;
            return MissionTransition::Continue(next_mode);
        }

        tokio::select! {
            ctrl = ctrl_rx.recv() => {
                match ctrl {
                    Some(MissionCtrl::Hold { reason }) => {
                        if let Some(active) = controller.take() {
                            active.stop();
                        }
                        flush_runtime_events(&mut runtime_rx, recorder, Duration::from_millis(250)).await;
                        recorder
                            .record_intervention("operator".to_string(), "hold".to_string(), Some(reason))
                            .await;
                        set_held_state(state, registry, status_tx, recorder).await;
                        return MissionTransition::Hold(MissionLoopMode::Launch {
                            override_retry_gate: false,
                        });
                    }
                    Some(MissionCtrl::Retry { actor, action, reason }) => {
                        if let Some(active) = controller.take() {
                            active.stop();
                        }
                        flush_runtime_events(&mut runtime_rx, recorder, Duration::from_millis(250)).await;
                        recorder.record_intervention(actor, action, Some(reason)).await;
                        return MissionTransition::Continue(MissionLoopMode::Launch {
                            override_retry_gate: true,
                        });
                    }
                    Some(MissionCtrl::Abort { actor, action, reason }) => {
                        if let Some(active) = controller.take() {
                            active.stop();
                        }
                        flush_runtime_events(&mut runtime_rx, recorder, Duration::from_millis(250)).await;
                        recorder.record_intervention(actor, action, Some(reason)).await;
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
    recorder: &mut MissionRecorder,
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
    recorder.record_scheduler_state(loop_mode_state(mode)).await;
}

async fn set_held_state(
    state: &Arc<Mutex<MissionState>>,
    registry: &Arc<Mutex<Registry>>,
    status_tx: &watch::Sender<Vec<MissionSummary>>,
    recorder: &mut MissionRecorder,
) {
    update_mission_state(state, registry, status_tx, |state| {
        state.state = MissionSchedulerState::Held;
        state.held = true;
        state.terminal = false;
        state.next_wake_at_ms = None;
    })
    .await;
    recorder.record_scheduler_state(MissionSchedulerState::Held).await;
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

const fn scheduler_state_name(state: MissionSchedulerState) -> &'static str {
    match state {
        MissionSchedulerState::Launching => "launching",
        MissionSchedulerState::Running => "running",
        MissionSchedulerState::Scheduled => "scheduled",
        MissionSchedulerState::Held => "held",
        MissionSchedulerState::AwaitingChange => "awaiting_change",
        MissionSchedulerState::AwaitingOperator => "awaiting_operator",
        MissionSchedulerState::TerminalFailure => "terminal_failure",
        MissionSchedulerState::Idle => "idle",
    }
}

const fn loop_mode_state(mode: &MissionLoopMode) -> MissionSchedulerState {
    match mode {
        MissionLoopMode::Launch { .. } => MissionSchedulerState::Launching,
        MissionLoopMode::Scheduled { .. } => MissionSchedulerState::Scheduled,
        MissionLoopMode::Idle => MissionSchedulerState::Idle,
        MissionLoopMode::AwaitingChange => MissionSchedulerState::AwaitingChange,
        MissionLoopMode::AwaitingOperator => MissionSchedulerState::AwaitingOperator,
        MissionLoopMode::TerminalFailure => MissionSchedulerState::TerminalFailure,
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

fn status_worthy_event(event: &Event) -> bool {
    !matches!(
        event,
        Event::Log { .. }
            | Event::GuestEventFwd { .. }
            | Event::KvGet { .. }
            | Event::KvSet { .. }
            | Event::KvDelete { .. }
            | Event::RequestDone { .. }
    )
}

fn push_capped<T>(items: &mut Vec<T>, item: T, max_len: usize) {
    items.push(item);
    if items.len() > max_len {
        let overflow = items.len().saturating_sub(max_len);
        items.drain(..overflow);
    }
}

#[cfg(test)]
mod tests;
