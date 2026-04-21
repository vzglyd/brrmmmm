use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use anyhow::{Context, Result};
use futures::FutureExt as _;
use wasmtime::{Engine, Module};

use crate::abi::{
    MissionModuleDescribe, MissionOutcome, MissionOutcomeStatus, MissionRuntimeState,
    PersistenceAuthority,
};
use crate::events::{Event, EventSink, diag, ms_to_iso8601, now_ms, now_ts};
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

struct RuntimeEnvironment {
    store: WasmStore,
    linker: WasmLinker,
    shared_host_state: Arc<Mutex<HostState>>,
}

struct DescribeState {
    entry_timeout_secs: u64,
    prior_ledger: Option<mission_ledger::MissionLedgerRecord>,
    input_fingerprint: Option<String>,
}

pub(super) async fn run_wasm_instance(
    engine: &Engine,
    module: &Module,
    config: WasmRunConfig,
    context: WasmRunContext,
    brrmmmm_config: &crate::config::Config,
) -> Result<()> {
    MissionRunner::new(config, context, brrmmmm_config)
        .run(engine, module)
        .await
}

struct MissionRunner<'a> {
    wasm_path: String,
    env_vars: Vec<(String, String)>,
    params_bytes: Option<Vec<u8>>,
    log_channel: bool,
    abi_version: u32,
    wasm_size_bytes: usize,
    wasm_hash: String,
    module_hash: ModuleHash,
    attestation_identity: Option<InstallationIdentity>,
    policy: RuntimePolicy,
    override_retry_gate: bool,
    artifact_store: Arc<Mutex<ArtifactStore>>,
    runtime_state: Arc<Mutex<MissionRuntimeState>>,
    params_state: Arc<Mutex<Option<Vec<u8>>>>,
    event_sink: EventSink,
    stop_signal: Arc<AtomicBool>,
    force_refresh: Arc<AtomicBool>,
    brrmmmm_config: &'a crate::config::Config,
}

impl<'a> MissionRunner<'a> {
    fn new(
        config: WasmRunConfig,
        context: WasmRunContext,
        brrmmmm_config: &'a crate::config::Config,
    ) -> Self {
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

        Self {
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
            artifact_store,
            runtime_state,
            params_state,
            event_sink,
            stop_signal,
            force_refresh,
            brrmmmm_config,
        }
    }

    async fn run(self, engine: &Engine, module: &Module) -> Result<()> {
        let mut environment = self.build_runtime_environment(engine)?;
        if let Err(error) =
            validate_params_contract(module, self.params_bytes.as_deref(), &self.policy)
        {
            self.fail_run(Some(&environment.shared_host_state), &error);
            return Err(error);
        }

        self.prepare_init_deadline(&mut environment);
        let instance = match self.instantiate_module(&mut environment, module).await {
            Ok(instance) => instance,
            Err(error) => {
                self.fail_run(Some(&environment.shared_host_state), &error);
                return Err(error);
            }
        };

        self.emit_started();
        let describe_state = match self
            .describe_run_context(
                &instance,
                &mut environment.store,
                &environment.shared_host_state,
            )
            .await
        {
            Ok(state) => state,
            Err(error) => {
                self.fail_run(Some(&environment.shared_host_state), &error);
                return Err(error);
            }
        };

        if self.apply_repeat_failure_gate_if_needed(&describe_state) {
            return Ok(());
        }

        self.wait_for_preflight_cooldown().await;
        self.reset_runtime_outcome_state();

        let call_result = self
            .execute_entrypoint(
                &instance,
                &mut environment.store,
                describe_state.entry_timeout_secs,
            )
            .await;
        self.finalize_run(
            &call_result,
            &environment.shared_host_state,
            &describe_state,
        );
        call_result
    }

    fn build_runtime_environment(&self, engine: &Engine) -> Result<RuntimeEnvironment> {
        let mut wasi_builder = wasmtime_wasi::WasiCtxBuilder::new();
        for (key, value) in &self.env_vars {
            let _ = wasi_builder.env(key, value);
        }
        wasi_builder.inherit_stdout().inherit_stderr();

        let store = build_wasm_store(engine, wasi_builder.build_p1(), &self.policy);
        let mut linker = WasmLinker::new(engine);
        wasmtime_wasi::preview1::add_to_linker_async(&mut linker, |state| &mut state.wasi)?;

        let mut host_state = HostState::new(
            self.log_channel,
            self.params_state.clone(),
            self.module_hash,
            self.attestation_identity.clone(),
            self.brrmmmm_config.clone(),
        );
        host_state.artifact_store = self.artifact_store.clone();

        let shared_host_state = register_brrmmmm_host_on_linker(
            &mut linker,
            host_state,
            self.event_sink.clone(),
            self.runtime_state.clone(),
            self.stop_signal.clone(),
            self.force_refresh.clone(),
            Some(self.wasm_hash.clone()),
            self.brrmmmm_config,
        )?;

        Ok(RuntimeEnvironment {
            store,
            linker,
            shared_host_state,
        })
    }

    fn prepare_init_deadline(&self, environment: &mut RuntimeEnvironment) {
        environment
            .store
            .set_epoch_deadline(self.policy.init_deadline_ticks());
        environment.store.epoch_deadline_trap();
    }

    async fn instantiate_module(
        &self,
        environment: &mut RuntimeEnvironment,
        module: &Module,
    ) -> Result<wasmtime::Instance> {
        run_guest_phase("instantiate WASM module", async {
            environment
                .linker
                .instantiate_async(&mut environment.store, module)
                .await
                .context("instantiate WASM module")
        })
        .await
    }

    fn emit_started(&self) {
        self.event_sink.emit(&Event::Started {
            ts: now_ts(),
            wasm_path: self.wasm_path.clone(),
            wasm_size_bytes: self.wasm_size_bytes,
            abi_version: self.abi_version,
        });
    }

    async fn describe_run_context(
        &self,
        instance: &wasmtime::Instance,
        store: &mut WasmStore,
        shared_host_state: &Arc<Mutex<HostState>>,
    ) -> Result<DescribeState> {
        let mut describe_state = DescribeState {
            entry_timeout_secs: self.policy.default_acquisition_timeout_secs,
            prior_ledger: None,
            input_fingerprint: None,
        };
        let describe_result = run_guest_phase(
            "read mission module describe",
            call_describe(instance, store, &self.policy),
        )
        .await;

        match describe_result {
            Ok(Some(describe)) => {
                describe_state.prior_ledger = mission_ledger::load(
                    self.brrmmmm_config,
                    &describe.logical_id,
                    self.module_hash,
                )
                .ok()
                .flatten();
                describe_state.entry_timeout_secs = describe
                    .acquisition_timeout_secs
                    .filter(|timeout_secs| *timeout_secs > 0)
                    .map_or(self.policy.default_acquisition_timeout_secs, u64::from);
                describe_state.input_fingerprint = Some(compute_input_fingerprint(
                    self.module_hash,
                    self.params_bytes.as_deref(),
                    &describe,
                    &self.env_vars,
                ));
                self.event_sink.emit(&Event::Describe {
                    ts: now_ts(),
                    describe: describe.clone(),
                });
                {
                    let mut host = lock_runtime(shared_host_state, "host_state");
                    host.set_mission_describe(&describe);
                }
                let mut state = lock_runtime(&self.runtime_state, "runtime_state");
                state.describe = Some(describe);
                if let Some(ledger) = describe_state.prior_ledger.as_ref() {
                    mission_ledger::apply_to_runtime_state(&mut state, ledger);
                }
                drop(state);
            }
            Ok(None) => diag(
                &self.event_sink,
                "[brrmmmm] mission module is missing static describe exports",
            ),
            Err(error) => return Err(error),
        }

        Ok(describe_state)
    }

    fn apply_repeat_failure_gate_if_needed(&self, describe_state: &DescribeState) -> bool {
        let Some(ledger) = describe_state.prior_ledger.as_ref() else {
            return false;
        };
        let Some(fingerprint) = describe_state.input_fingerprint.as_deref() else {
            return false;
        };
        if self.override_retry_gate
            || !mission_ledger::repeat_failure_gate_active(ledger, fingerprint)
        {
            return false;
        }

        let outcome = changed_conditions_required_outcome(ledger);
        if let Err(error) = update_mission_outcome_state(
            &self.runtime_state,
            &self.event_sink,
            outcome,
            "host",
            &self.brrmmmm_config.assurance,
        ) {
            diag(
                &self.event_sink,
                &format!("[brrmmmm] failed to apply repeat-failure gate: {error}"),
            );
        }
        self.event_sink.emit(&Event::ModuleExit {
            ts: now_ts(),
            reason: "repeat_failure_gate".to_string(),
        });
        persist_runtime_and_ledger(
            &self.runtime_state,
            &self.event_sink,
            self.brrmmmm_config,
            &self.wasm_hash,
            self.module_hash,
            describe_state.input_fingerprint.as_deref(),
            describe_state.prior_ledger.as_ref(),
        );
        true
    }

    async fn wait_for_preflight_cooldown(&self) {
        let cooldown_until_ms =
            lock_runtime(&self.runtime_state, "runtime_state").cooldown_until_ms;
        let Some(until_ms) = cooldown_until_ms else {
            return;
        };
        let now = now_ms();
        if until_ms <= now {
            return;
        }

        let wait_ms = until_ms - now;
        diag(
            &self.event_sink,
            &format!("[brrmmmm] pre-flight cooldown: waiting {wait_ms}ms before starting"),
        );
        self.event_sink.emit(&Event::SleepStart {
            ts: now_ts(),
            duration_ms: i64::try_from(wait_ms).unwrap_or(i64::MAX),
            wake_at: ms_to_iso8601(until_ms),
        });
        tokio::time::sleep(std::time::Duration::from_millis(wait_ms)).await;
    }

    fn reset_runtime_outcome_state(&self) {
        let mut state = lock_runtime(&self.runtime_state, "runtime_state");
        state.last_outcome = None;
        state.last_outcome_at_ms = None;
        state.last_outcome_reported_by = None;
        state.last_host_decision = None;
        state.pending_operator_action = None;
    }

    async fn execute_entrypoint(
        &self,
        instance: &wasmtime::Instance,
        store: &mut WasmStore,
        entry_timeout_secs: u64,
    ) -> Result<()> {
        store.set_epoch_deadline(RuntimePolicy::acquisition_deadline_ticks(
            entry_timeout_secs,
        ));
        store.epoch_deadline_trap();
        diag(
            &self.event_sink,
            &format!(
                "[brrmmmm] acquisition budget of {entry_timeout_secs}s enforced via epoch interrupt"
            ),
        );
        diag(&self.event_sink, "[brrmmmm] starting mission module...");

        let entry_name =
            find_entry(instance, store).context("WASM module has no recognised entry point")?;
        let entry = instance
            .get_func(&mut *store, &entry_name)
            .with_context(|| format!("get entry function: {entry_name}"))?;
        diag(
            &self.event_sink,
            &format!("[brrmmmm] calling entry '{entry_name}' (runs until stopped)"),
        );

        run_guest_phase("execute mission module entry", async {
            entry
                .call_async(store, &[], &mut [])
                .await
                .with_context(|| format!("execute entry function: {entry_name}"))
        })
        .await
    }

    fn finalize_run(
        &self,
        call_result: &Result<()>,
        shared_host_state: &Arc<Mutex<HostState>>,
        describe_state: &DescribeState,
    ) {
        let reason = match call_result {
            Ok(()) => "completed",
            Err(error) if is_host_import_panic(error) => "host_import_panic",
            Err(error) if is_timeout_error(error) => "timeout",
            Err(_) => "error",
        };
        ensure_terminal_outcome(
            &self.runtime_state,
            &self.event_sink,
            &self.artifact_store,
            call_result.as_ref().err(),
            &self.brrmmmm_config.assurance,
        );
        self.event_sink.emit(&Event::ModuleExit {
            ts: now_ts(),
            reason: reason.to_string(),
        });

        if let Err(error) = call_result
            && is_host_import_panic(error)
        {
            mark_runtime_corrupted(
                &self.stop_signal,
                &self.artifact_store,
                Some(shared_host_state),
                &self.event_sink,
            );
        }

        if call_result.as_ref().err().is_some_and(is_host_import_panic) {
            diag(
                &self.event_sink,
                "[brrmmmm] skipped runtime state persistence after host import panic",
            );
            return;
        }

        persist_runtime_and_ledger(
            &self.runtime_state,
            &self.event_sink,
            self.brrmmmm_config,
            &self.wasm_hash,
            self.module_hash,
            describe_state.input_fingerprint.as_deref(),
            describe_state.prior_ledger.as_ref(),
        );
    }

    fn fail_run(&self, host_state: Option<&Arc<Mutex<HostState>>>, error: &anyhow::Error) {
        finish_failed_run(
            &self.runtime_state,
            &self.event_sink,
            &self.stop_signal,
            &self.artifact_store,
            host_state,
            error,
            &self.brrmmmm_config.assurance,
        );
    }
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
        .map(std::string::String::as_str)
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
    ensure_terminal_outcome(
        runtime_state,
        event_sink,
        artifact_store,
        Some(error),
        assurance,
    );
    event_sink.emit(&Event::ModuleExit {
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

    if let Err(error) =
        update_mission_outcome_state(runtime_state, event_sink, synthesized, "host", assurance)
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
        .is_some_and(|describe| describe.state_persistence == PersistenceAuthority::HostPersisted);

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
        .or_else(|| {
            ledger
                .last_outcome
                .as_ref()
                .map(|outcome| outcome.reason_code.as_str())
        })
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
mod tests;
