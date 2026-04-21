//! Mission-module inspection, validation, and runtime control APIs.

mod host_imports;
mod inspection;
mod io;
mod runner;

pub use inspection::{MissionInspection, inspect_module_contract, validate_module_inspection};

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread;

use anyhow::{Context, Result};
use wasmtime::{Config as WasmtimeConfig, Engine, Module};

use crate::abi::{ABI_VERSION_V4, ActiveMode, MissionOutcome, MissionPhase, MissionRuntimeState};
use crate::config::Config;
use crate::events::EventSink;
use crate::host::ArtifactStore;
use crate::identity;
use crate::persistence;

use io::RuntimePolicy;
use io::lock_runtime;
use runner::{WasmRunConfig, WasmRunContext, run_wasm_instance};

// ── MissionController ────────────────────────────────────────────────

/// Final terminal state observed for one mission run.
#[derive(Debug, Clone)]
pub struct MissionCompletion {
    /// Final typed mission outcome.
    pub outcome: MissionOutcome,
    /// Final runtime snapshot observed by the host.
    pub snapshot: MissionRuntimeState,
    /// Last raw-source artifact observed during this mission, when any.
    pub raw_source: Option<Vec<u8>>,
    /// Last normalized artifact observed during this mission, when any.
    pub normalized: Option<Vec<u8>>,
    /// Published output artifact when one was produced.
    pub published_output: Option<Vec<u8>>,
}

/// Owns a running WASM mission module and provides an observable runtime state.
pub struct MissionController {
    /// Canonical runtime state; read by `snapshot()`.
    runtime_state: Arc<Mutex<MissionRuntimeState>>,
    /// Named artifact store; published_output consumed by `--once` mode.
    artifact_store: Arc<Mutex<ArtifactStore>>,
    /// Background thread running the mission module.
    thread: Option<thread::JoinHandle<()>>,
    /// Signal to gracefully stop the mission-module thread.
    stop_signal: Arc<AtomicBool>,
    /// When set, the next `announce_sleep` call returns 1 (skip sleep).
    force_refresh: Arc<AtomicBool>,
    /// Current JSON params exposed through host `params_len`/`params_read` imports.
    params_bytes: Arc<Mutex<Option<Vec<u8>>>>,
}

impl MissionController {
    /// Load a mission-module WASM module and start running it in a background thread.
    pub fn new(
        wasm_path: &str,
        env_vars: Vec<(String, String)>,
        params_bytes: Option<Vec<u8>>,
        log_channel: bool,
        override_retry_gate: bool,
        event_sink: EventSink,
        config: &Config,
    ) -> Result<Self> {
        let policy = RuntimePolicy::from_limits(&config.limits);
        if let Some(params) = params_bytes.as_ref()
            && params.len() > policy.max_params_bytes
        {
            anyhow::bail!(
                "mission params are {} bytes, exceeding the configured limit of {} bytes",
                params.len(),
                policy.max_params_bytes
            );
        }

        let wasm_bytes =
            std::fs::read(wasm_path).with_context(|| format!("read WASM file: {wasm_path}"))?;

        let wasm_hash = persistence::wasm_identity(&wasm_bytes);
        let module_hash = identity::ModuleHash(crate::utils::sha256_digest(&wasm_bytes));
        let attestation_identity = if config.attestation_disabled {
            None
        } else {
            Some(identity::load_or_create(config).context(
                "load or create brrmmmm attestation identity; set BRRMMMM_ATTESTATION=off for explicit legacy mode",
            )?)
        };
        let mut runtime_state = persistence::load(config, &wasm_hash)
            .with_context(|| format!("load persisted runtime state for WASM {wasm_hash}"))?
            .unwrap_or_default();
        runtime_state.phase = MissionPhase::Idle;
        runtime_state.last_error = None;
        runtime_state.last_outcome = None;
        runtime_state.last_outcome_at_ms = None;
        runtime_state.last_outcome_reported_by = None;
        runtime_state.last_host_decision = None;
        runtime_state.pending_operator_action = None;
        runtime_state.last_raw_artifact = None;
        runtime_state.last_output_artifact = None;
        let runtime_state = Arc::new(Mutex::new(runtime_state));
        let stop_signal = Arc::new(AtomicBool::new(false));

        let mut engine_config = WasmtimeConfig::new();
        engine_config.epoch_interruption(true);
        engine_config.async_support(true);
        let engine = Engine::new(&engine_config).context("create wasmtime engine")?;
        let module = Module::from_binary(&engine, &wasm_bytes)
            .with_context(|| format!("compile WASM module: {wasm_path}"))?;

        // Shared epoch timer: increments engine epoch every 100ms so that
        // store.set_epoch_deadline() provides hard timeout enforcement regardless
        // of whether the guest cooperatively checks the stop_signal.
        let engine_for_timer = engine.clone();
        let stop_for_timer = stop_signal.clone();
        thread::spawn(move || {
            while !stop_for_timer.load(Ordering::Relaxed) {
                thread::sleep(std::time::Duration::from_millis(100));
                engine_for_timer.increment_epoch();
            }
        });

        {
            lock_runtime(&runtime_state, "runtime_state").mode = ActiveMode::ManagedPolling;
        }

        // Build shared stores.
        let artifact_store = Arc::new(Mutex::new(ArtifactStore::default()));
        let force_refresh = Arc::new(AtomicBool::new(false));
        let params_state = Arc::new(Mutex::new(params_bytes.clone()));
        let artifact_store_clone = artifact_store.clone();
        let force_refresh_clone = force_refresh.clone();
        let params_state_clone = params_state.clone();
        let runtime_state_clone = runtime_state.clone();
        let stop_clone = stop_signal.clone();
        let wasm_path_str = wasm_path.to_string();
        let config_clone = config.clone();
        let wasm_bytes_for_thread = wasm_bytes.clone();

        let handle = thread::spawn(move || {
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    eprintln!("[brrmmmm] failed to build tokio runtime: {error}");
                    return;
                }
            };
            let config = WasmRunConfig {
                wasm_path: wasm_path_str,
                env_vars,
                params_bytes,
                log_channel,
                abi_version: ABI_VERSION_V4,
                wasm_size_bytes: wasm_bytes_for_thread.len(),
                wasm_hash,
                module_hash,
                attestation_identity,
                policy,
                override_retry_gate,
            };
            let context = WasmRunContext {
                artifact_store: artifact_store_clone,
                runtime_state: runtime_state_clone,
                params_state: params_state_clone,
                event_sink,
                stop_signal: stop_clone,
                force_refresh: force_refresh_clone,
            };
            let result = runtime.block_on(run_wasm_instance(
                &engine,
                &module,
                config,
                context,
                &config_clone,
            ));
            if let Err(e) = result {
                eprintln!("[brrmmmm] WASM execution error: {e:?}");
            }
        });

        Ok(Self {
            runtime_state,
            artifact_store,
            thread: Some(handle),
            stop_signal,
            force_refresh,
            params_bytes: params_state,
        })
    }

    /// Return a snapshot of the current runtime state.
    pub fn snapshot(&self) -> MissionRuntimeState {
        lock_runtime(&self.runtime_state, "runtime_state").clone()
    }

    /// Return the mission-module-declared acquisition budget once describe() has been read.
    pub fn acquisition_timeout_secs(&self) -> Option<u32> {
        self.snapshot()
            .describe
            .and_then(|describe| describe.acquisition_timeout_secs)
    }

    /// Poll for the terminal mission outcome, consuming the published output artifact if present.
    pub fn poll_completion(&self) -> Option<MissionCompletion> {
        let snapshot = self.snapshot();
        let outcome = snapshot.last_outcome.clone()?;
        let (raw_source, normalized, published_output) = {
            let mut store = lock_runtime(&self.artifact_store, "artifact_store");
            (
                store
                    .raw_source
                    .as_ref()
                    .map(|artifact| artifact.data.clone()),
                store
                    .normalized
                    .as_ref()
                    .map(|artifact| artifact.data.clone()),
                store.take_published().map(|artifact| artifact.data),
            )
        };
        Some(MissionCompletion {
            outcome,
            snapshot,
            raw_source,
            normalized,
            published_output,
        })
    }

    /// Return a clone of the force-refresh flag for use by the stdin command listener.
    pub fn force_refresh_flag(&self) -> Arc<AtomicBool> {
        self.force_refresh.clone()
    }

    /// Return a clone of the current params handle for command listeners.
    pub fn params_handle(&self) -> Arc<Mutex<Option<Vec<u8>>>> {
        self.params_bytes.clone()
    }

    /// Signal the mission module to stop and detach the background thread.
    pub fn stop(mut self) {
        self.stop_signal.store(true, Ordering::Relaxed);
        self.thread.take();
    }
}

impl Drop for MissionController {
    fn drop(&mut self) {
        self.stop_signal.store(true, Ordering::Relaxed);
        self.thread.take();
        // runner handles saving state on exit, but we could do it here too if needed
    }
}
