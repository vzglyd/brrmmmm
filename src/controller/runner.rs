use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use anyhow::{Context, Result};
use futures::FutureExt as _;
use wasmtime::{Engine, Module};

use crate::abi::{MissionModuleDescribe, MissionOutcome, MissionOutcomeStatus, MissionRuntimeState, PersistenceAuthority};
use crate::events::{Event, EventSink, diag, now_ts};
use crate::host::{ArtifactStore, HostState};
use crate::identity::{InstallationIdentity, ModuleHash};
use crate::mission_ledger;
use crate::persistence;
use crate::utils::{base64url, sha256_digest};

use super::host_imports::register_brrmmmm_host_on_linker;
use super::inspection::read_static_describe;
use super::io::{
    RuntimePolicy, WasmLinker, WasmStore, build_wasm_store, lock_runtime,
    update_mission_outcome_state,
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
    pub(super) override_retry_gate: bool,
}

pub(super) struct WasmRunContext {
    pub(super) artifact_store: Arc<Mutex<ArtifactStore>>,
    pub(super) runtime_state: Arc<Mutex<MissionRuntimeState>>,
    pub(super) params_state: Arc<Mutex<Option<Vec<u8>>>>,
    pub(super) event_sink: EventSink,
    pub(super) stop_signal: Arc<AtomicBool>,
    pub(super) force_refresh: Arc<AtomicBool>,
}

pub(super) async fn run_wasm_instance(
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
        override_retry_gate,
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
    wasmtime_wasi::preview1::add_to_linker_async(&mut linker, |state| &mut state.wasi)?;

    // Build host state and register all brrmmmm_host imports.
    let mut host_state = HostState::new(
        log_channel,
        params_state,
        module_hash,
        attestation_identity,
        brrmmmm_config.clone(),
    );
    // Share the artifact_store from the controller.
    host_state.artifact_store = artifact_store.clone();

    let shared_host_state = register_brrmmmm_host_on_linker(
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
            &brrmmmm_config.assurance,
        );
        return Err(error);
    }

    // With epoch_interruption enabled, the default deadline is 0 (immediately interruptible).
    // Start the init budget only when guest work can actually begin.
    store.set_epoch_deadline(policy.init_deadline_ticks());
    store.epoch_deadline_trap();

    let instance = match run_guest_phase("instantiate WASM module", async {
        linker
            .instantiate_async(&mut store, module)
            .await
            .context("instantiate WASM module")
    })
    .await
    {
        Ok(instance) => instance,
        Err(error) => {
            finish_failed_run(
                &runtime_state,
                &event_sink,
                &stop_signal,
                &artifact_store,
                Some(&shared_host_state),
                &error,
                &brrmmmm_config.assurance,
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
    let mut prior_ledger = None;
    let mut input_fingerprint = None;
    let describe_result = run_guest_phase(
        "read mission module describe",
        call_describe(&instance, &mut store, &policy),
    )
    .await;

    match describe_result {
        Ok(Some(describe)) => {
            let ledger = mission_ledger::load(brrmmmm_config, &describe.logical_id, module_hash)
                .ok()
                .flatten();
            entry_timeout_secs = describe
                .acquisition_timeout_secs
                .filter(|timeout_secs| *timeout_secs > 0)
                .map(u64::from)
                .unwrap_or(policy.default_acquisition_timeout_secs);
            let fingerprint =
                compute_input_fingerprint(module_hash, params_bytes.as_deref(), &describe, &env_vars);
            event_sink.emit(Event::Describe {
                ts: now_ts(),
                describe: describe.clone(),
            });
            {
                let mut host = lock_runtime(&shared_host_state, "host_state");
                host.set_mission_describe(&describe);
            }
            {
                let mut state = lock_runtime(&runtime_state, "runtime_state");
                state.describe = Some(describe.clone());
                if let Some(ledger) = ledger.as_ref() {
                    mission_ledger::apply_to_runtime_state(&mut state, &ledger);
                }
            }
            prior_ledger = ledger;
            input_fingerprint = Some(fingerprint);
        }
        Ok(None) => diag(
            &event_sink,
            "[brrmmmm] mission module is missing static describe exports",
        ),
        Err(error) => {
            finish_failed_run(
                &runtime_state,
                &event_sink,
                &stop_signal,
                &artifact_store,
                Some(&shared_host_state),
                &error,
                &brrmmmm_config.assurance,
            );
            return Err(error);
        }
    }

    if let (Some(ledger), Some(fingerprint)) = (prior_ledger.as_ref(), input_fingerprint.as_deref())
        && mission_ledger::repeat_failure_gate_active(ledger, fingerprint)
        && !override_retry_gate
    {
        let outcome = changed_conditions_required_outcome(ledger);
        if let Err(error) = update_mission_outcome_state(
            &runtime_state,
            &event_sink,
            outcome,
            "host",
            &brrmmmm_config.assurance,
        ) {
            diag(
                &event_sink,
                &format!("[brrmmmm] failed to apply repeat-failure gate: {error}"),
            );
        }
        event_sink.emit(Event::ModuleExit {
            ts: now_ts(),
            reason: "repeat_failure_gate".to_string(),
        });
        persist_runtime_and_ledger(
            &runtime_state,
            &event_sink,
            brrmmmm_config,
            &wasm_hash,
            module_hash,
            input_fingerprint.as_deref(),
            prior_ledger.as_ref(),
        );
        return Ok(());
    }

    {
        let mut state = lock_runtime(&runtime_state, "runtime_state");
        // Continuity from the ledger informs retries and cooldowns, but a fresh
        // attempt must not begin with the previous attempt's terminal outcome.
        state.last_outcome = None;
        state.last_outcome_at_ms = None;
        state.last_outcome_reported_by = None;
        state.last_host_decision = None;
        state.pending_operator_action = None;
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

    diag(&event_sink, "[brrmmmm] starting mission module...");

    let entry_name = find_entry(&instance, &mut store);

    let entry_name = entry_name.context("WASM module has no recognised entry point")?;
    let entry = instance
        .get_func(&mut store, &entry_name)
        .with_context(|| format!("get entry function: {entry_name}"))?;

    diag(
        &event_sink,
        &format!("[brrmmmm] calling entry '{entry_name}' (runs until stopped)"),
    );

    let call_result = run_guest_phase("execute mission module entry", async {
        entry
            .call_async(&mut store, &[], &mut [])
            .await
            .with_context(|| format!("execute entry function: {entry_name}"))
    })
    .await;

    let reason = match &call_result {
        Ok(_) => "completed",
        Err(e) if is_host_import_panic(e) => "host_import_panic",
        Err(e) if is_timeout_error(e) => "timeout",
        Err(_) => "error",
    };
    ensure_terminal_outcome(
        &runtime_state,
        &event_sink,
        &artifact_store,
        call_result.as_ref().err(),
        &brrmmmm_config.assurance,
    );
    event_sink.emit(Event::ModuleExit {
        ts: now_ts(),
        reason: reason.to_string(),
    });

    if let Err(error) = &call_result
        && is_host_import_panic(error)
    {
        mark_runtime_corrupted(
            &stop_signal,
            &artifact_store,
            Some(&shared_host_state),
            &event_sink,
        );
    }

    if !call_result.as_ref().err().is_some_and(is_host_import_panic) {
        persist_runtime_and_ledger(
            &runtime_state,
            &event_sink,
            brrmmmm_config,
            &wasm_hash,
            module_hash,
            input_fingerprint.as_deref(),
            prior_ledger.as_ref(),
        );
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

async fn run_guest_phase<T>(
    phase: &'static str,
    future: impl std::future::Future<Output = Result<T>>,
) -> Result<T> {
    match std::panic::AssertUnwindSafe(future).catch_unwind().await {
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
    runtime_state: &Arc<Mutex<MissionRuntimeState>>,
    event_sink: &EventSink,
    stop_signal: &Arc<AtomicBool>,
    artifact_store: &Arc<Mutex<ArtifactStore>>,
    host_state: Option<&Arc<Mutex<HostState>>>,
    error: &anyhow::Error,
    assurance: &crate::config::RuntimeAssurance,
) {
    let reason = if is_host_import_panic(error) {
        mark_runtime_corrupted(stop_signal, artifact_store, host_state, event_sink);
        "host_import_panic"
    } else if is_timeout_error(error) {
        "timeout"
    } else {
        "error"
    };
    ensure_terminal_outcome(runtime_state, event_sink, artifact_store, Some(error), assurance);
    event_sink.emit(Event::ModuleExit {
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
            "mission params are {} bytes, exceeding the configured limit of {} bytes",
            params.len(),
            policy.max_params_bytes
        );
    }

    if module_imports_host_params(module) {
        return Ok(());
    }

    anyhow::bail!(
        "mission params were provided, but the module does not import brrmmmm_host.params_len and brrmmmm_host.params_read"
    );
}

fn module_imports_host_params(module: &Module) -> bool {
    let mut has_len = false;
    let mut has_read = false;
    for import in module.imports() {
        if import.module() != "brrmmmm_host" {
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

fn find_entry(instance: &wasmtime::Instance, store: &mut WasmStore) -> Option<String> {
    if instance
        .get_func(&mut *store, "brrmmmm_module_start")
        .is_some()
    {
        return Some("brrmmmm_module_start".to_string());
    }
    None
}

// ── describe() call ──────────────────────────────────────────────────

async fn call_describe(
    instance: &wasmtime::Instance,
    store: &mut WasmStore,
    policy: &RuntimePolicy,
) -> Result<Option<crate::abi::MissionModuleDescribe>> {
    read_static_describe(instance, store, policy).await
}

fn ensure_terminal_outcome(
    runtime_state: &Arc<Mutex<MissionRuntimeState>>,
    event_sink: &EventSink,
    artifact_store: &Arc<Mutex<ArtifactStore>>,
    error: Option<&anyhow::Error>,
    assurance: &crate::config::RuntimeAssurance,
) {
    if lock_runtime(runtime_state, "runtime_state")
        .last_outcome
        .is_some()
    {
        return;
    }

    let artifact_kind = lock_runtime(artifact_store, "artifact_store")
        .published_output
        .as_ref()
        .map(|artifact| artifact.kind.clone());

    let synthesized = if let Some(error) = error {
        if is_timeout_error(error) {
            MissionOutcome {
                status: MissionOutcomeStatus::RetryableFailure,
                reason_code: "acquisition_timeout".to_string(),
                message: format!("{error:#}"),
                retry_after_ms: None,
                operator_action: None,
                operator_timeout_ms: None,
                operator_timeout_outcome: None,
                primary_artifact_kind: artifact_kind,
            }
        } else {
            MissionOutcome {
                status: MissionOutcomeStatus::TerminalFailure,
                reason_code: "runtime_error".to_string(),
                message: format!("{error:#}"),
                retry_after_ms: None,
                operator_action: None,
                operator_timeout_ms: None,
                operator_timeout_outcome: None,
                primary_artifact_kind: artifact_kind,
            }
        }
    } else if artifact_kind.is_some() {
        MissionOutcome {
            status: MissionOutcomeStatus::Published,
            reason_code: "published_output".to_string(),
            message: "mission module published an output artifact".to_string(),
            retry_after_ms: None,
            operator_action: None,
            operator_timeout_ms: None,
            operator_timeout_outcome: None,
            primary_artifact_kind: artifact_kind,
        }
    } else {
        MissionOutcome {
            status: MissionOutcomeStatus::TerminalFailure,
            reason_code: "missing_outcome_report".to_string(),
            message: "mission module exited without reporting a final outcome".to_string(),
            retry_after_ms: None,
            operator_action: None,
            operator_timeout_ms: None,
            operator_timeout_outcome: None,
            primary_artifact_kind: None,
        }
    };

    if let Err(error) = update_mission_outcome_state(
        runtime_state,
        event_sink,
        synthesized,
        "host",
        assurance,
    )
    {
        diag(
            event_sink,
            &format!("[brrmmmm] failed to record synthesized mission outcome: {error}"),
        );
    }
}

fn persist_runtime_and_ledger(
    runtime_state: &Arc<Mutex<MissionRuntimeState>>,
    event_sink: &EventSink,
    brrmmmm_config: &crate::config::Config,
    wasm_hash: &str,
    module_hash: ModuleHash,
    input_fingerprint: Option<&str>,
    prior_ledger: Option<&mission_ledger::MissionLedgerRecord>,
) {
    let state = lock_runtime(runtime_state, "runtime_state");
    let should_persist = state
        .describe
        .as_ref()
        .map(|describe| describe.state_persistence == PersistenceAuthority::HostPersisted)
        .unwrap_or(false);

    if should_persist && let Err(error) = persistence::save(brrmmmm_config, wasm_hash, &state) {
        diag(
            event_sink,
            &format!("[brrmmmm] failed to persist runtime state: {error:#}"),
        );
    }
    if let Some(describe) = state.describe.as_ref()
        && let Err(error) = mission_ledger::save(
            brrmmmm_config,
            &describe.logical_id,
            module_hash,
            &state,
            input_fingerprint,
            prior_ledger,
            brrmmmm_config.assurance.same_reason_retry_limit,
        )
    {
        diag(
            event_sink,
            &format!("[brrmmmm] failed to persist mission ledger: {error:#}"),
        );
    }
}

fn compute_input_fingerprint(
    module_hash: ModuleHash,
    params_bytes: Option<&[u8]>,
    describe: &MissionModuleDescribe,
    env_vars: &[(String, String)],
) -> String {
    let mut material = Vec::new();
    material.extend_from_slice(module_hash.as_bytes());
    material.extend_from_slice(b"\0params\0");
    material.extend_from_slice(params_bytes.unwrap_or_default());

    let env_map = env_vars
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut env_names = describe
        .required_env_vars
        .iter()
        .chain(describe.optional_env_vars.iter())
        .map(|env| env.name.as_str())
        .collect::<Vec<_>>();
    env_names.sort_unstable();
    env_names.dedup();

    for name in env_names {
        material.extend_from_slice(b"\0env\0");
        material.extend_from_slice(name.as_bytes());
        material.extend_from_slice(b"\0");
        if let Some(value) = env_map.get(name) {
            material.extend_from_slice(value.as_bytes());
        } else {
            material.extend_from_slice(b"<unset>");
        }
    }

    base64url(&sha256_digest(&material))
}

fn changed_conditions_required_outcome(
    ledger: &mission_ledger::MissionLedgerRecord,
) -> MissionOutcome {
    let blocked_reason = ledger
        .repeat_failure_gate
        .as_ref()
        .map(|gate| gate.reason_code.as_str())
        .or_else(|| ledger.last_outcome.as_ref().map(|outcome| outcome.reason_code.as_str()))
        .unwrap_or("repeated_failure");

    MissionOutcome {
        status: MissionOutcomeStatus::RetryableFailure,
        reason_code: "changed_conditions_required".to_string(),
        message: format!(
            "automation already hit the same failure ({blocked_reason}) with unchanged inputs; change the inputs, environment, or module before launching another automated attempt"
        ),
        retry_after_ms: None,
        operator_action: None,
        operator_timeout_ms: None,
        operator_timeout_outcome: None,
        primary_artifact_kind: None,
    }
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
    use crate::abi::MissionRuntimeState;

    fn describe_json(
        acquisition_timeout_secs: Option<u32>,
        operator_fallback: Option<&str>,
    ) -> String {
        let acquisition = acquisition_timeout_secs
            .map(|value| value.to_string())
            .unwrap_or_else(|| "null".to_string());
        let operator_fallback = operator_fallback.unwrap_or("null");
        format!(
            r#"{{"schema_version":1,"logical_id":"test.mission","name":"Test Mission Module","description":"test mission module","abi_version":4,"run_modes":["managed_polling"],"state_persistence":"volatile","required_env_vars":[],"optional_env_vars":[],"params":{{"fields":[]}},"capabilities_needed":[],"poll_strategy":null,"cooldown_policy":null,"artifact_types":["published_output"],"acquisition_timeout_secs":{acquisition},"operator_fallback":{operator_fallback}}}"#
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
                (func (export "brrmmmm_module_abi_version") (result i32)
                    i32.const 4)
                (func (export "brrmmmm_module_describe_ptr") (result i32)
                    i32.const 16)
                (func (export "brrmmmm_module_describe_len") (result i32)
                    i32.const {describe_len})
                (func (export "brrmmmm_module_start")
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
        Arc<Mutex<MissionRuntimeState>>,
        Arc<Mutex<ArtifactStore>>,
    ) {
        let mut engine_config = Config::new();
        engine_config.epoch_interruption(true);
        engine_config.async_support(true);
        let engine = Engine::new(&engine_config).expect("test engine");
        let module = Module::new(&engine, wat).expect("test module");

        let artifact_store = Arc::new(Mutex::new(ArtifactStore::default()));
        let runtime_state = Arc::new(Mutex::new(MissionRuntimeState::default()));
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

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test tokio runtime");
        let result = runtime.block_on(run_wasm_instance(
            &engine,
            &module,
            WasmRunConfig {
                wasm_path: "test.wasm".to_string(),
                env_vars: Vec::new(),
                params_bytes,
                log_channel: false,
                abi_version: 4,
                wasm_size_bytes: wat.len(),
                wasm_hash: "test-wasm".to_string(),
                module_hash: ModuleHash([0u8; 32]),
                attestation_identity: None,
                policy,
                override_retry_gate: false,
            },
            WasmRunContext {
                artifact_store: artifact_store.clone(),
                runtime_state: runtime_state.clone(),
                params_state,
                event_sink,
                stop_signal: stop_signal.clone(),
                force_refresh,
            },
            &crate::config::Config::load().expect("test config"),
        ));

        stop_signal.store(true, Ordering::Relaxed);
        let _ = timer.join();
        (result, runtime_state, artifact_store)
    }

    #[test]
    fn params_are_read_through_host_owned_imports() {
        let params = br#"{"location":"Daylesford"}"#.to_vec();
        let wat = wat_module(
            r#"
                (import "brrmmmm_host" "params_len" (func $params_len (result i32)))
                (import "brrmmmm_host" "params_read" (func $params_read (param i32 i32) (result i32)))
                (import "brrmmmm_host" "artifact_publish" (func $artifact_publish (param i32 i32 i32 i32) (result i32)))
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
            &describe_json(None, None),
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
    fn params_without_host_imports_are_rejected() {
        let wat = wat_module("", "", &describe_json(None, None));

        let (result, runtime_state, _artifact_store) =
            run_test_wat(&wat, Some(br#"{"x":1}"#.to_vec()), RuntimePolicy::default());

        let error = result.expect_err("params should fail without host imports");
        let error_message = error.to_string();
        assert!(error_message.contains("does not import brrmmmm_host.params_len"));
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
            "(module (func (export \"brrmmmm_module_start\")))",
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
            &describe_json(None, None),
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
            &describe_json(None, None),
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
    fn operator_rescue_outcome_sets_pending_escalation_state() {
        let outcome = r#"{"status":"operator_action_required","reason_code":"captcha_blocked","message":"automation exhausted","operator_action":"Complete the upstream login challenge.","operator_timeout_ms":50,"operator_timeout_outcome":"retryable_failure"}"#;
        let describe = describe_json(
            None,
            Some(r#"{"timeout_ms":60000,"on_timeout":"terminal_failure"}"#),
        );
        let outcome_len = outcome.len();
        let wat = format!(
            r#"(module
                (import "brrmmmm_host" "mission_outcome_report" (func $mission_outcome_report (param i32 i32) (result i32)))
                (memory (export "memory") 1)
                (data (i32.const 16) "{describe_data}")
                (data (i32.const 2048) "{outcome_data}")
                (func (export "brrmmmm_module_abi_version") (result i32)
                    i32.const 4)
                (func (export "brrmmmm_module_describe_ptr") (result i32)
                    i32.const 16)
                (func (export "brrmmmm_module_describe_len") (result i32)
                    i32.const {describe_len})
                (func (export "brrmmmm_module_start")
                    i32.const 2048
                    i32.const {outcome_len}
                    call $mission_outcome_report
                    drop)
            )"#,
            describe_data = wat_bytes(describe.as_bytes()),
            describe_len = describe.len(),
            outcome_data = wat_bytes(outcome.as_bytes()),
            outcome_len = outcome_len,
        );

        let (result, runtime_state, _artifact_store) =
            run_test_wat(&wat, None, RuntimePolicy::default());

        assert!(result.is_ok(), "run failed: {result:?}");
        let state = lock_runtime(&runtime_state, "runtime_state").clone();
        assert_eq!(
            state.last_outcome.expect("outcome").status,
            MissionOutcomeStatus::OperatorActionRequired
        );
        let escalation = state.pending_operator_action.expect("pending escalation");
        assert_eq!(escalation.action, "Complete the upstream login challenge.");
        assert_eq!(
            escalation.timeout_outcome.mission_status(),
            MissionOutcomeStatus::RetryableFailure
        );
        assert!(escalation.deadline_at_ms >= state.last_outcome_at_ms.unwrap_or_default());
    }

    #[test]
    fn operator_rescue_without_declared_fallback_is_rejected() {
        let outcome = r#"{"status":"operator_action_required","reason_code":"captcha_blocked","message":"automation exhausted","operator_action":"Complete the upstream login challenge."}"#;
        let describe = describe_json(None, None);
        let outcome_len = outcome.len();
        let wat = format!(
            r#"(module
                (import "brrmmmm_host" "mission_outcome_report" (func $mission_outcome_report (param i32 i32) (result i32)))
                (memory (export "memory") 1)
                (data (i32.const 16) "{describe_data}")
                (data (i32.const 2048) "{outcome_data}")
                (func (export "brrmmmm_module_abi_version") (result i32)
                    i32.const 4)
                (func (export "brrmmmm_module_describe_ptr") (result i32)
                    i32.const 16)
                (func (export "brrmmmm_module_describe_len") (result i32)
                    i32.const {describe_len})
                (func (export "brrmmmm_module_start")
                    i32.const 2048
                    i32.const {outcome_len}
                    call $mission_outcome_report
                    drop)
            )"#,
            describe_data = wat_bytes(describe.as_bytes()),
            describe_len = describe.len(),
            outcome_data = wat_bytes(outcome.as_bytes()),
            outcome_len = outcome_len,
        );

        let (result, runtime_state, _artifact_store) =
            run_test_wat(&wat, None, RuntimePolicy::default());

        assert!(result.is_ok(), "run failed: {result:?}");
        let state = lock_runtime(&runtime_state, "runtime_state").clone();
        let outcome = state.last_outcome.expect("terminal outcome");
        assert_eq!(outcome.status, MissionOutcomeStatus::TerminalFailure);
        assert_eq!(outcome.reason_code, "mission_protocol_error");
        assert!(state.pending_operator_action.is_none());
    }

    #[test]
    fn panic_payloads_are_preserved_and_truncated() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test tokio runtime");
        let result = runtime.block_on(run_guest_phase::<()>("test import", async {
            std::panic::panic_any("x".repeat(600));
            #[allow(unreachable_code)]
            Ok(())
        }));

        let error = result.expect_err("panic should be converted to error");
        let panic = error
            .downcast_ref::<HostImportPanic>()
            .expect("panic error type");
        assert_eq!(panic.phase, "test import");
        assert!(panic.message.ends_with("..."));
        assert!(panic.message.len() < 600);
    }
}
