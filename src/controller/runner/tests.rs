use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use wasmtime::{Config, Engine, Module};

use super::*;
use crate::abi::MissionRuntimeState;
use crate::events::now_ms;

fn temp_dir(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "brrmmmm-runner-test-{label}-{}-{}",
        std::process::id(),
        now_ms()
    ))
}

fn describe_json(acquisition_timeout_secs: Option<u32>, operator_fallback: Option<&str>) -> String {
    let acquisition =
        acquisition_timeout_secs.map_or_else(|| "null".to_string(), |value| value.to_string());
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
    let temp_root = temp_dir("runtime");
    let mut config = crate::config::Config::load().expect("test config");
    config.identity_dir = temp_root.join("identity");
    config.state_dir = temp_root.join("state");
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
        &config,
    ));

    stop_signal.store(true, Ordering::Relaxed);
    let _ = timer.join();
    let _ = std::fs::remove_dir_all(temp_root);
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
        r"
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
        ",
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
        r"
            i32.const 1
            memory.grow
            drop
        ",
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
        r"
            (loop $spin
                br $spin)
        ",
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
