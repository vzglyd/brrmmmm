use brrmmmm::abi::{
    ActiveMode, CooldownPolicy, PersistenceAuthority, PollStrategy, SidecarDescribe, SidecarPhase,
    SidecarRuntimeState,
};

fn roundtrip<T: serde::Serialize + for<'de> serde::Deserialize<'de>>(value: &T) -> String {
    let json = serde_json::to_string(value).unwrap();
    let _: T = serde_json::from_str(&json).unwrap();
    json
}

#[test]
fn sidecar_phase_roundtrips_all_variants() {
    for phase in [
        SidecarPhase::Idle,
        SidecarPhase::CoolingDown,
        SidecarPhase::Fetching,
        SidecarPhase::Parsing,
        SidecarPhase::Publishing,
        SidecarPhase::Failed,
    ] {
        let json = roundtrip(&phase);
        let decoded: SidecarPhase = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, phase);
    }
}

#[test]
fn persistence_authority_roundtrips_all_variants() {
    for authority in [
        PersistenceAuthority::Volatile,
        PersistenceAuthority::HostPersisted,
        PersistenceAuthority::VendorBacked,
    ] {
        let json = roundtrip(&authority);
        let decoded: PersistenceAuthority = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, authority);
    }
}

#[test]
fn active_mode_roundtrips_all_variants() {
    for mode in [ActiveMode::ManagedPolling, ActiveMode::Interactive] {
        let json = roundtrip(&mode);
        let decoded: ActiveMode = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, mode);
    }
}

#[test]
fn poll_strategy_fixed_interval_roundtrip() {
    let strategy = PollStrategy::FixedInterval { interval_secs: 30 };
    let json = roundtrip(&strategy);
    assert!(json.contains("30"));
}

#[test]
fn poll_strategy_exponential_backoff_roundtrip() {
    let strategy = PollStrategy::ExponentialBackoff {
        base_secs: 5,
        max_secs: 300,
    };
    let json = roundtrip(&strategy);
    assert!(json.contains("5"));
    assert!(json.contains("300"));
}

#[test]
fn poll_strategy_jittered_roundtrip() {
    let strategy = PollStrategy::Jittered {
        base_secs: 60,
        jitter_secs: 10,
    };
    let json = roundtrip(&strategy);
    assert!(json.contains("60"));
    assert!(json.contains("10"));
}

#[test]
fn poll_strategy_display_fixed_interval() {
    let strategy = PollStrategy::FixedInterval { interval_secs: 30 };
    assert_eq!(strategy.to_string(), "fixed_interval 30s");
}

#[test]
fn poll_strategy_display_exponential_backoff() {
    let strategy = PollStrategy::ExponentialBackoff {
        base_secs: 5,
        max_secs: 300,
    };
    assert_eq!(strategy.to_string(), "exponential_backoff base=5s max=300s");
}

#[test]
fn poll_strategy_display_jittered() {
    let strategy = PollStrategy::Jittered {
        base_secs: 60,
        jitter_secs: 10,
    };
    assert_eq!(strategy.to_string(), "jittered base=60s jitter=10s");
}

#[test]
fn cooldown_policy_roundtrip() {
    let policy = CooldownPolicy {
        authority: PersistenceAuthority::HostPersisted,
        min_interval_ms: 5000,
    };
    let json = roundtrip(&policy);
    let decoded: CooldownPolicy = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded.min_interval_ms, 5000);
    assert_eq!(decoded.authority, PersistenceAuthority::HostPersisted);
}

#[test]
fn sidecar_runtime_state_default_roundtrip() {
    let state = SidecarRuntimeState::default();
    let json = serde_json::to_string(&state).unwrap();
    let decoded: SidecarRuntimeState = serde_json::from_str(&json).unwrap();
    let json2 = serde_json::to_string(&decoded).unwrap();
    assert_eq!(json, json2);
}

#[test]
fn sidecar_describe_acquisition_timeout_defaults_to_none() {
    let describe: SidecarDescribe = serde_json::from_value(serde_json::json!({
        "schema_version": 1,
        "logical_id": "brrmmmm.test",
        "name": "Test Sidecar",
        "description": "Test sidecar",
        "abi_version": 1,
        "run_modes": ["managed_polling"],
        "state_persistence": "volatile",
        "required_env_vars": [],
        "optional_env_vars": [],
        "params": {"fields": []},
        "capabilities_needed": [],
        "poll_strategy": null,
        "cooldown_policy": null,
        "artifact_types": ["published_output"]
    }))
    .unwrap();

    assert_eq!(describe.acquisition_timeout_secs, None);
}

#[test]
fn sidecar_describe_acquisition_timeout_roundtrips() {
    let describe: SidecarDescribe = serde_json::from_value(serde_json::json!({
        "schema_version": 1,
        "logical_id": "brrmmmm.test",
        "name": "Test Sidecar",
        "description": "Test sidecar",
        "abi_version": 1,
        "run_modes": ["managed_polling"],
        "state_persistence": "volatile",
        "required_env_vars": [],
        "optional_env_vars": [],
        "params": {"fields": []},
        "capabilities_needed": ["browser", "ai"],
        "acquisition_timeout_secs": 90,
        "poll_strategy": null,
        "cooldown_policy": null,
        "artifact_types": ["published_output"]
    }))
    .unwrap();

    let json = roundtrip(&describe);
    let decoded: SidecarDescribe = serde_json::from_str(&json).unwrap();

    assert_eq!(decoded.acquisition_timeout_secs, Some(90));
}
