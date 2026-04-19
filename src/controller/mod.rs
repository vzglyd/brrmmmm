mod host_imports;
mod inspection;
mod io;
mod runner;

pub use inspection::{SidecarInspection, inspect_wasm_contract, validate_inspection};

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread;

use anyhow::{Context, Result};
use wasmtime::{Engine, Module};

use crate::abi::{ABI_VERSION_V1, ActiveMode, SidecarRuntimeState};
use crate::events::EventSink;
use crate::host::ArtifactStore;
use crate::persistence;

use io::lock_runtime;
use runner::{WasmRunConfig, WasmRunContext, run_wasm_instance};

// ── SidecarController ────────────────────────────────────────────────

/// Owns a running WASM sidecar module and provides an observable runtime state.
pub struct SidecarController {
    /// Canonical runtime state; read by `snapshot()`.
    runtime_state: Arc<Mutex<SidecarRuntimeState>>,
    /// Named artifact store; published_output consumed by `--once` mode.
    artifact_store: Arc<Mutex<ArtifactStore>>,
    /// Background thread running the sidecar.
    thread: Option<thread::JoinHandle<()>>,
    /// Signal to gracefully stop the sidecar thread.
    stop_signal: Arc<AtomicBool>,
    /// When set, the next `announce_sleep` call returns 1 (skip sleep).
    force_refresh: Arc<AtomicBool>,
    /// Current JSON params exposed through host `params_len`/`params_read` imports.
    params_bytes: Arc<Mutex<Option<Vec<u8>>>>,
}

impl SidecarController {
    /// Load a sidecar WASM module and start running it in a background thread.
    pub fn new(
        wasm_path: &str,
        env_vars: Vec<(String, String)>,
        params_bytes: Option<Vec<u8>>,
        log_channel: bool,
        event_sink: EventSink,
    ) -> Result<Self> {
        let wasm_bytes =
            std::fs::read(wasm_path).with_context(|| format!("read WASM file: {wasm_path}"))?;

        let wasm_hash = persistence::wasm_identity(&wasm_bytes);
        let runtime_state = persistence::load(&wasm_hash).unwrap_or_default();
        let runtime_state = Arc::new(Mutex::new(runtime_state));
        let stop_signal = Arc::new(AtomicBool::new(false));

        let engine = Engine::default();
        let module = Module::from_binary(&engine, &wasm_bytes)
            .with_context(|| format!("compile WASM module: {wasm_path}"))?;

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

        let handle = thread::spawn(move || {
            let config = WasmRunConfig {
                wasm_path: wasm_path_str,
                env_vars,
                params_bytes,
                log_channel,
                abi_version: ABI_VERSION_V1,
                wasm_size_bytes: wasm_bytes.len(),
                wasm_hash,
            };
            let context = WasmRunContext {
                artifact_store: artifact_store_clone,
                runtime_state: runtime_state_clone,
                params_state: params_state_clone,
                event_sink,
                stop_signal: stop_clone,
                force_refresh: force_refresh_clone,
            };
            let result = run_wasm_instance(&engine, &module, config, context);
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
    pub fn snapshot(&self) -> SidecarRuntimeState {
        lock_runtime(&self.runtime_state, "runtime_state").clone()
    }

    /// Return the sidecar-declared acquisition budget once describe() has been read.
    pub fn acquisition_timeout_secs(&self) -> Option<u32> {
        self.snapshot()
            .describe
            .and_then(|describe| describe.acquisition_timeout_secs)
    }

    /// Poll for the latest published_output artifact, consuming it.
    pub fn poll_output(&self) -> Option<Vec<u8>> {
        lock_runtime(&self.artifact_store, "artifact_store")
            .take_published()
            .map(|a| a.data)
    }

    /// Return a clone of the force-refresh flag for use by the stdin command listener.
    pub fn force_refresh_flag(&self) -> Arc<AtomicBool> {
        self.force_refresh.clone()
    }

    /// Return a clone of the current params handle for command listeners.
    pub fn params_handle(&self) -> Arc<Mutex<Option<Vec<u8>>>> {
        self.params_bytes.clone()
    }

    /// Signal the sidecar to stop and detach the background thread.
    pub fn stop(mut self) {
        self.stop_signal.store(true, Ordering::Relaxed);
        self.thread.take();
    }
}

impl Drop for SidecarController {
    fn drop(&mut self) {
        self.stop_signal.store(true, Ordering::Relaxed);
        self.thread.take();
    }
}
