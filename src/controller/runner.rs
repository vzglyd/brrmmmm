use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use anyhow::{Context, Result};
use wasmtime::{Engine, Module};

use crate::abi::{PersistenceAuthority, SidecarRuntimeState};
use crate::events::{Event, EventSink, diag, now_ts};
use crate::host::{ArtifactStore, HostState};
use crate::identity::{InstallationIdentity, ModuleHash};
use crate::persistence;

use super::host_imports::register_vzglyd_host_on_linker;
use super::inspection::read_static_describe;
use super::io::{
    RuntimePolicy, WasmLinker, WasmStore, build_wasm_store, lock_runtime, update_failure_state,
};

// ── WASM instance runner ─────────────────────────────────────────────

pub(super) struct WasmRunConfig {
    pub(super) wasm_path: String,
    pub(super) env_vars: Vec<(String, String)>,
    pub(super) params_bytes: Option<Vec<u8>>,
    pub(super) log_channel: bool,
    pub(super) abi_version: u32,
    pub(super) wasm_size_bytes: usize,
    pub(super) wasm_hash: String,
    pub(super) module_hash: ModuleHash,
    pub(super) attestation_identity: Option<InstallationIdentity>,
    pub(super) policy: RuntimePolicy,
}

pub(super) struct WasmRunContext {
    pub(super) artifact_store: Arc<Mutex<ArtifactStore>>,
    pub(super) runtime_state: Arc<Mutex<SidecarRuntimeState>>,
    pub(super) params_state: Arc<Mutex<Option<Vec<u8>>>>,
    pub(super) event_sink: EventSink,
    pub(super) stop_signal: Arc<AtomicBool>,
    pub(super) force_refresh: Arc<AtomicBool>,
}

pub(super) fn run_wasm_instance(
    engine: &Engine,
    module: &Module,
    config: WasmRunConfig,
    context: WasmRunContext,
    brrmmmm_config: &crate::config::Config,
) -> Result<()> {
    let WasmRunConfig {
        wasm_path,
        env_vars,
        params_bytes,
        log_channel,
        abi_version,
        wasm_size_bytes,
        wasm_hash,
        module_hash,
        attestation_identity,
        policy,
    } = config;
    let WasmRunContext {
        artifact_store,
        runtime_state,
        params_state,
        event_sink,
        stop_signal,
        force_refresh,
    } = context;

    // Build WASI preview1 context.
    let mut wasi_builder = wasmtime_wasi::WasiCtxBuilder::new();
    for (key, value) in &env_vars {
        let _ = wasi_builder.env(key, value);
    }
    wasi_builder.inherit_stdout().inherit_stderr();
    let wasi_p1 = wasi_builder.build_p1();

    let mut store = build_wasm_store(engine, wasi_p1, &policy);
    let mut linker = WasmLinker::new(engine);
    wasmtime_wasi::preview1::add_to_linker_sync(&mut linker, |state| &mut state.wasi)?;

    // Build host state and register all vzglyd_host imports.
    let mut host_state = HostState::new(
        log_channel,
        params_state,
        module_hash,
        attestation_identity,
        brrmmmm_config.clone(),
    );
    // Share the artifact_store from the controller.
    host_state.artifact_store = artifact_store.clone();

    let shared_host_state = register_vzglyd_host_on_linker(
        &mut linker,
        host_state,
        event_sink.clone(),
        runtime_state.clone(),
        stop_signal.clone(),
        force_refresh,
        Some(wasm_hash.clone()),
        brrmmmm_config,
    )?;

    if let Err(error) = validate_params_contract(module, params_bytes.as_deref(), &policy) {
        finish_failed_run(
            &runtime_state,
            &event_sink,
            &stop_signal,
            &artifact_store,
            Some(&shared_host_state),
            &error,
        );
        return Err(error);
    }

    // With epoch_interruption enabled, the default deadline is 0 (immediately interruptible).
    // Start the init budget only when guest work can actually begin.
    store.set_epoch_deadline(policy.init_deadline_ticks());
    store.epoch_deadline_trap();

    let instance = match run_guest_phase("instantiate WASM module", || {
        linker
            .instantiate(&mut store, module)
            .context("instantiate WASM module")
    }) {
        Ok(instance) => instance,
        Err(error) => {
            finish_failed_run(
                &runtime_state,
                &event_sink,
                &stop_signal,
                &artifact_store,
                Some(&shared_host_state),
                &error,
            );
            return Err(error);
        }
    };

    // Emit Started event (before calling the entry point).
    event_sink.emit(Event::Started {
        ts: now_ts(),
        wasm_path: wasm_path.clone(),
        wasm_size_bytes,
        abi_version,
    });

    let mut entry_timeout_secs = policy.default_acquisition_timeout_secs;
    let describe_result = run_guest_phase("read sidecar describe", || {
        call_describe(&instance, &mut store, &policy)
    });

    match describe_result {
        Ok(Some(describe)) => {
            entry_timeout_secs = describe
                .acquisition_timeout_secs
                .filter(|timeout_secs| *timeout_secs > 0)
                .map(u64::from)
                .unwrap_or(policy.default_acquisition_timeout_secs);
            event_sink.emit(Event::Describe {
                ts: now_ts(),
                describe: describe.clone(),
            });
            {
                let mut host = lock_runtime(&shared_host_state, "host_state");
                host.set_mission_describe(&describe);
            }
            lock_runtime(&runtime_state, "runtime_state").describe = Some(describe);
        }
        Ok(None) => diag(
            &event_sink,
            "[brrmmmm] sidecar is missing static describe exports",
        ),
        Err(error) => {
            finish_failed_run(
                &runtime_state,
                &event_sink,
                &stop_signal,
                &artifact_store,
                Some(&shared_host_state),
                &error,
            );
            return Err(error);
        }
    }

    // Set hard epoch deadline: epoch timer increments at ~10 Hz (100ms intervals),
    // so timeout_secs * 10 ticks gives the acquisition budget as a hard cutoff
    // that fires even if the guest is in a tight compute loop.
    store.set_epoch_deadline(policy.acquisition_deadline_ticks(entry_timeout_secs));
    store.epoch_deadline_trap();
    diag(
        &event_sink,
        &format!(
            "[brrmmmm] acquisition budget of {entry_timeout_secs}s enforced via epoch interrupt"
        ),
    );

    diag(&event_sink, "[brrmmmm] starting sidecar...");

    let entry_name = find_entry(&instance, &mut store);

    let entry_name = entry_name.context("WASM module has no recognised entry point")?;
    let entry = instance
        .get_func(&mut store, &entry_name)
        .with_context(|| format!("get entry function: {entry_name}"))?;

    diag(
        &event_sink,
        &format!("[brrmmmm] calling entry '{entry_name}' (runs until stopped)"),
    );

    let call_result = run_guest_phase("execute sidecar entry", || {
        entry
            .call(&mut store, &[], &mut [])
            .with_context(|| format!("execute entry function: {entry_name}"))
    });

    let reason = match &call_result {
        Ok(_) => "completed",
        Err(e) if is_host_import_panic(e) => "host_import_panic",
        Err(e) if is_timeout_error(e) => "timeout",
        Err(_) => "error",
    };
    event_sink.emit(Event::SidecarExit {
        ts: now_ts(),
        reason: reason.to_string(),
    });

    if let Err(error) = &call_result {
        update_failure_state(&runtime_state, &error.to_string());
        if is_host_import_panic(error) {
            mark_runtime_corrupted(
                &stop_signal,
                &artifact_store,
                Some(&shared_host_state),
                &event_sink,
            );
        }
    }

    // Save persisted state if the sidecar requests it.
    if !call_result.as_ref().err().is_some_and(is_host_import_panic) {
        let state = lock_runtime(&runtime_state, "runtime_state");
        let should_persist = state
            .describe
            .as_ref()
            .map(|d| d.state_persistence == PersistenceAuthority::HostPersisted)
            .unwrap_or(false);

        if should_persist {
            if let Err(error) = persistence::save(brrmmmm_config, &wasm_hash, &state) {
                diag(
                    &event_sink,
                    &format!("[brrmmmm] failed to persist runtime state: {error:#}"),
                );
            }
        }
    } else {
        diag(
            &event_sink,
            "[brrmmmm] skipped runtime state persistence after host import panic",
        );
    }

    call_result
}

// ── Run failure handling ─────────────────────────────────────────────

#[derive(Debug)]
struct HostImportPanic {
    phase: &'static str,
    message: String,
}

impl std::fmt::Display for HostImportPanic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "host import panic during {}: {}",
            self.phase, self.message
        )
    }
}

impl std::error::Error for HostImportPanic {}

fn run_guest_phase<T>(phase: &'static str, f: impl FnOnce() -> Result<T>) -> Result<T> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(result) => result,
        Err(panic_payload) => Err(anyhow::Error::new(HostImportPanic {
            phase,
            message: panic_payload_message(panic_payload.as_ref()),
        })),
    }
}

fn panic_payload_message(panic_payload: &(dyn std::any::Any + Send)) -> String {
    let msg = panic_payload
        .downcast_ref::<String>()
        .map(|s| s.as_str())
        .or_else(|| panic_payload.downcast_ref::<&str>().copied())
        .unwrap_or("non-string panic payload");
    truncate_diagnostic(msg)
}

fn truncate_diagnostic(msg: &str) -> String {
    const MAX_CHARS: usize = 512;
    let mut out = String::new();
    for (index, ch) in msg.chars().enumerate() {
        if index == MAX_CHARS {
            out.push_str("...");
            return out;
        }
        out.push(ch);
    }
    out
}

fn is_host_import_panic(error: &anyhow::Error) -> bool {
    error.downcast_ref::<HostImportPanic>().is_some()
}

fn is_timeout_error(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<wasmtime::Trap>(),
        Some(wasmtime::Trap::Interrupt)
    )
}

fn finish_failed_run(
    runtime_state: &Arc<Mutex<SidecarRuntimeState>>,
    event_sink: &EventSink,
    stop_signal: &Arc<AtomicBool>,
    artifact_store: &Arc<Mutex<ArtifactStore>>,
    host_state: Option<&Arc<Mutex<HostState>>>,
    error: &anyhow::Error,
) {
    let reason = if is_host_import_panic(error) {
        mark_runtime_corrupted(stop_signal, artifact_store, host_state, event_sink);
        "host_import_panic"
    } else if is_timeout_error(error) {
        "timeout"
    } else {
        "error"
    };
    update_failure_state(runtime_state, &error.to_string());
    event_sink.emit(Event::SidecarExit {
        ts: now_ts(),
        reason: reason.to_string(),
    });
}

fn mark_runtime_corrupted(
    stop_signal: &Arc<AtomicBool>,
    artifact_store: &Arc<Mutex<ArtifactStore>>,
    host_state: Option<&Arc<Mutex<HostState>>>,
    event_sink: &EventSink,
) {
    stop_signal.store(true, Ordering::Relaxed);
    lock_runtime(artifact_store, "artifact_store").clear();
    if let Some(host_state) = host_state {
        lock_runtime(host_state, "host_state").clear_transient_runtime_outputs();
    }
    diag(
        event_sink,
        "[brrmmmm] runtime marked corrupted after host import panic; transient state cleared",
    );
}

// ── Parameter ABI validation ─────────────────────────────────────────

fn validate_params_contract(
    module: &Module,
    params_bytes: Option<&[u8]>,
    policy: &RuntimePolicy,
) -> Result<()> {
    let Some(params) = params_bytes else {
        return Ok(());
    };

    if params.len() > policy.max_params_bytes {
        anyhow::bail!(
            "sidecar params are {} bytes, exceeding the configured limit of {} bytes",
            params.len(),
            policy.max_params_bytes
        );
    }

    if module_imports_host_params(module) {
        return Ok(());
    }

    if module_exports_legacy_configure(module) {
        let mode = if policy.legacy_configure_buffer {
            "configured but disabled in this build"
        } else {
            "disabled"
        };
        anyhow::bail!(
            "sidecar exposes legacy vzglyd_params_ptr/capacity/configure exports, but the raw configure buffer is {mode}; use vzglyd_host.params_len/params_read for production params"
        );
    }

    anyhow::bail!(
        "sidecar params were provided, but the module does not import vzglyd_host.params_len and vzglyd_host.params_read"
    );
}

fn module_imports_host_params(module: &Module) -> bool {
    let mut has_len = false;
    let mut has_read = false;
    for import in module.imports() {
        if import.module() != "vzglyd_host" {
            continue;
        }
        match import.name() {
            "params_len" => has_len = true,
            "params_read" => has_read = true,
            _ => {}
        }
    }
    has_len && has_read
}

fn module_exports_legacy_configure(module: &Module) -> bool {
    [
        "vzglyd_params_ptr",
        "vzglyd_params_capacity",
        "vzglyd_configure",
    ]
    .iter()
    .any(|name| module.get_export(name).is_some())
}

// ── Entry point resolution ───────────────────────────────────────────

fn find_entry(instance: &wasmtime::Instance, store: &mut WasmStore) -> Option<String> {
    for name in &["vzglyd_sidecar_start", "_start", "main"] {
        if instance.get_func(&mut *store, name).is_some() {
            return Some(name.to_string());
        }
    }
    None
}

// ── describe() call ──────────────────────────────────────────────────

fn call_describe(
    instance: &wasmtime::Instance,
    store: &mut WasmStore,
    policy: &RuntimePolicy,
) -> Result<Option<crate::abi::SidecarDescribe>> {
    read_static_describe(instance, store, policy)
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    };
    use std::time::Duration;

    use wasmtime::{Config, Engine, Module};

    use super::*;
    use crate::abi::SidecarRuntimeState;

    fn describe_json(acquisition_timeout_secs: Option<u32>) -> String {
        let acquisition = acquisition_timeout_secs
            .map(|value| value.to_string())
            .unwrap_or_else(|| "null".to_string());
        format!(
            r#"{{"schema_version":1,"logical_id":"test.sidecar","name":"Test Sidecar","description":"test sidecar","abi_version":1,"run_modes":["managed_polling"],"state_persistence":"volatile","required_env_vars":[],"optional_env_vars":[],"params":{{"fields":[]}},"capabilities_needed":[],"poll_strategy":null,"cooldown_policy":null,"artifact_types":["published_output"],"acquisition_timeout_secs":{acquisition}}}"#
        )
    }

    fn wat_bytes(bytes: &[u8]) -> String {
        bytes
            .iter()
            .map(|byte| match *byte {
                b' '..=b'!' | b'#'..=b'[' | b']'..=b'~' => (*byte as char).to_string(),
                _ => format!("\\{byte:02x}"),
            })
            .collect::<String>()
    }

    fn wat_module(imports: &str, start_body: &str, describe: &str) -> String {
        let describe_len = describe.len();
        let describe_data = wat_bytes(describe.as_bytes());
        format!(
            r#"(module
                {imports}
                (memory (export "memory") 1)
                (data (i32.const 16) "{describe_data}")
                (data (i32.const 1024) "published_output")
                (func (export "vzglyd_sidecar_abi_version") (result i32)
                    i32.const 1)
                (func (export "vzglyd_sidecar_describe_ptr") (result i32)
                    i32.const 16)
                (func (export "vzglyd_sidecar_describe_len") (result i32)
                    i32.const {describe_len})
                (func (export "vzglyd_sidecar_start")
                    {start_body})
            )"#
        )
    }

    fn run_test_wat(
        wat: &str,
        params_bytes: Option<Vec<u8>>,
        policy: RuntimePolicy,
    ) -> (
        Result<()>,
        Arc<Mutex<SidecarRuntimeState>>,
        Arc<Mutex<ArtifactStore>>,
    ) {
        let mut engine_config = Config::new();
        engine_config.epoch_interruption(true);
        let engine = Engine::new(&engine_config).expect("test engine");
        let module = Module::new(&engine, wat).expect("test module");

        let artifact_store = Arc::new(Mutex::new(ArtifactStore::default()));
        let runtime_state = Arc::new(Mutex::new(SidecarRuntimeState::default()));
        let params_state = Arc::new(Mutex::new(params_bytes.clone()));
        let event_sink = EventSink::noop();
        let stop_signal = Arc::new(AtomicBool::new(false));
        let force_refresh = Arc::new(AtomicBool::new(false));

        let engine_for_timer = engine.clone();
        let stop_for_timer = stop_signal.clone();
        let timer = std::thread::spawn(move || {
            while !stop_for_timer.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(1));
                engine_for_timer.increment_epoch();
            }
        });

        let result = run_wasm_instance(
            &engine,
            &module,
            WasmRunConfig {
                wasm_path: "test.wasm".to_string(),
                env_vars: Vec::new(),
                params_bytes,
                log_channel: false,
                abi_version: 1,
                wasm_size_bytes: wat.len(),
                wasm_hash: "test-wasm".to_string(),
                module_hash: ModuleHash([0u8; 32]),
                attestation_identity: None,
                policy,
            },
            WasmRunContext {
                artifact_store: artifact_store.clone(),
                runtime_state: runtime_state.clone(),
                params_state,
                event_sink,
                stop_signal: stop_signal.clone(),
                force_refresh,
            },
            &crate::config::Config::load(),
        );

        stop_signal.store(true, Ordering::Relaxed);
        let _ = timer.join();
        (result, runtime_state, artifact_store)
    }

    #[test]
    fn params_are_read_through_host_owned_imports() {
        let params = br#"{"location":"Daylesford"}"#.to_vec();
        let wat = wat_module(
            r#"
                (import "vzglyd_host" "params_len" (func $params_len (result i32)))
                (import "vzglyd_host" "params_read" (func $params_read (param i32 i32) (result i32)))
                (import "vzglyd_host" "artifact_publish" (func $artifact_publish (param i32 i32 i32 i32) (result i32)))
            "#,
            r#"
                (local $len i32)
                local.get $len
                drop
                call $params_len
                local.set $len
                i32.const 2048
                local.get $len
                call $params_read
                drop
                i32.const 1024
                i32.const 16
                i32.const 2048
                local.get $len
                call $artifact_publish
                drop
            "#,
            &describe_json(None),
        );

        let (result, _runtime_state, artifact_store) =
            run_test_wat(&wat, Some(params.clone()), RuntimePolicy::default());

        assert!(result.is_ok(), "run failed: {result:?}");
        let published = lock_runtime(&artifact_store, "artifact_store")
            .published_output
            .as_ref()
            .map(|artifact| artifact.data.clone());
        assert_eq!(published, Some(params));
    }

    #[test]
    fn params_with_legacy_configure_exports_are_rejected() {
        let wat = wat_module("", "", &describe_json(None)).replace(
            "(func (export \"vzglyd_sidecar_start\")",
            r#"(func (export "vzglyd_params_ptr") (result i32) i32.const 2048)
                (func (export "vzglyd_params_capacity") (result i32) i32.const 64)
                (func (export "vzglyd_configure") (param i32) (result i32) i32.const 0)
                (func (export "vzglyd_sidecar_start")"#,
        );

        let (result, runtime_state, _artifact_store) =
            run_test_wat(&wat, Some(br#"{"x":1}"#.to_vec()), RuntimePolicy::default());

        let error = result.expect_err("legacy params should fail");
        let error_message = error.to_string();
        assert!(error_message.contains("legacy vzglyd_params_ptr"));
        assert_eq!(
            lock_runtime(&runtime_state, "runtime_state")
                .last_error
                .as_deref(),
            Some(error_message.as_str())
        );
    }

    #[test]
    fn oversized_params_are_rejected_before_guest_execution() {
        let module = Module::new(
            &Engine::default(),
            "(module (func (export \"vzglyd_sidecar_start\")))",
        )
        .expect("test module");
        let policy = RuntimePolicy {
            max_params_bytes: 4,
            ..RuntimePolicy::default()
        };

        let result = validate_params_contract(&module, Some(b"too large"), &policy);

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("exceeding"));
    }

    #[test]
    fn memory_growth_beyond_policy_limit_fails() {
        let policy = RuntimePolicy {
            max_wasm_memory_bytes: 64 * 1024,
            ..RuntimePolicy::default()
        };
        let wat = wat_module(
            "",
            r#"
                i32.const 1
                memory.grow
                drop
            "#,
            &describe_json(None),
        );

        let (result, runtime_state, _artifact_store) = run_test_wat(&wat, None, policy);

        let error = result.expect_err("memory growth should fail");
        assert!(format!("{error:#}").contains("memory"));
        assert!(
            lock_runtime(&runtime_state, "runtime_state")
                .last_error
                .as_ref()
                .is_some()
        );
    }

    #[test]
    fn epoch_interrupt_is_classified_as_timeout() {
        let policy = RuntimePolicy {
            default_acquisition_timeout_secs: 1,
            ..RuntimePolicy::default()
        };
        let wat = wat_module(
            "",
            r#"
                (loop $spin
                    br $spin)
            "#,
            &describe_json(None),
        );

        let (result, runtime_state, _artifact_store) = run_test_wat(&wat, None, policy);

        let error = result.expect_err("spin loop should time out");
        assert!(is_timeout_error(&error), "unexpected error: {error:#}");
        assert!(
            lock_runtime(&runtime_state, "runtime_state")
                .last_error
                .as_ref()
                .is_some()
        );
    }

    #[test]
    fn panic_payloads_are_preserved_and_truncated() {
        let result = run_guest_phase::<()>("test import", || {
            std::panic::panic_any("x".repeat(600));
        });

        let error = result.expect_err("panic should be converted to error");
        let panic = error
            .downcast_ref::<HostImportPanic>()
            .expect("panic error type");
        assert_eq!(panic.phase, "test import");
        assert!(panic.message.ends_with("..."));
        assert!(panic.message.len() < 600);
    }
}
