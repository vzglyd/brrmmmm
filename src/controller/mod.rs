mod host_imports;
mod inspection;
mod io;
mod runner;

pub use inspection::{
    SidecarInspection, detect_abi_version, inspect_wasm_contract, validate_inspection,
};

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread;

use anyhow::{Context, Result};
use wasmtime::{Engine, Module};

use crate::abi::{ABI_VERSION_V2, ActiveMode, SidecarRuntimeState};
use crate::events::EventSink;
use crate::host::ArtifactStore;

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

        let runtime_state = Arc::new(Mutex::new(SidecarRuntimeState::default()));
        let stop_signal = Arc::new(AtomicBool::new(false));

        let engine = Engine::default();
        let module = Module::from_binary(&engine, &wasm_bytes)
            .with_context(|| format!("compile WASM module: {wasm_path}"))?;

        // Detect ABI version by checking for the v2 export.
        let abi_version = detect_abi_version(&module);

        // Update mode in runtime state.
        {
            let mut state = runtime_state.lock().expect("mutex poisoned");
            state.mode = match abi_version {
                ABI_VERSION_V2 => ActiveMode::ManagedPolling,
                _ => ActiveMode::V1Legacy,
            };
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
                abi_version,
                wasm_size_bytes: wasm_bytes.len(),
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
        self.runtime_state.lock().expect("mutex poisoned").clone()
    }

    /// Poll for the latest published_output artifact, consuming it.
    pub fn poll_output(&self) -> Option<Vec<u8>> {
        self.artifact_store
            .lock()
            .expect("mutex poisoned")
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
