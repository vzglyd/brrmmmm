use brrmmmm::abi::{
    ActiveMode, ArtifactMeta, CooldownPolicy, EnvVarSpec, GuestEvent, PersistenceAuthority,
    PollStrategy, SidecarDescribe, SidecarParamField, SidecarParamOption, SidecarParamType,
    SidecarParamsSchema, SidecarPhase, SidecarRuntimeState,
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
fn artifact_meta_roundtrip() {
    let meta = ArtifactMeta {
        kind: "published_output".to_string(),
        size_bytes: 1024,
        received_at_ms: 1_700_000_000_000,
    };
    let json = roundtrip(&meta);
    let decoded: ArtifactMeta = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded.kind, "published_output");
    assert_eq!(decoded.size_bytes, 1024);
    assert_eq!(decoded.received_at_ms, 1_700_000_000_000);
}

#[test]
fn guest_event_roundtrip_with_attrs() {
    let event = GuestEvent {
        ts_ms: 1_700_000_000_000,
        kind: "poll_complete".to_string(),
        attrs: serde_json::json!({"count": 5}),
    };
    let json = roundtrip(&event);
    let decoded: GuestEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded.kind, "poll_complete");
    assert_eq!(decoded.ts_ms, 1_700_000_000_000);
}

#[test]
fn guest_event_attrs_defaults_to_null_when_missing() {
    let json = r#"{"ts_ms": 1000, "kind": "ping"}"#;
    let decoded: GuestEvent = serde_json::from_str(json).unwrap();
    assert_eq!(decoded.attrs, serde_json::Value::Null);
}

#[test]
fn env_var_spec_roundtrip() {
    let spec = EnvVarSpec {
        name: "API_KEY".to_string(),
        description: "The API key".to_string(),
    };
    let json = roundtrip(&spec);
    let decoded: EnvVarSpec = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded.name, "API_KEY");
    assert_eq!(decoded.description, "The API key");
}

#[test]
fn sidecar_param_type_serializes_as_snake_case() {
    assert_eq!(
        serde_json::to_string(&SidecarParamType::String).unwrap(),
        r#""string""#
    );
    assert_eq!(
        serde_json::to_string(&SidecarParamType::Integer).unwrap(),
        r#""integer""#
    );
    assert_eq!(
        serde_json::to_string(&SidecarParamType::Number).unwrap(),
        r#""number""#
    );
    assert_eq!(
        serde_json::to_string(&SidecarParamType::Boolean).unwrap(),
        r#""boolean""#
    );
    assert_eq!(
        serde_json::to_string(&SidecarParamType::Json).unwrap(),
        r#""json""#
    );
}

#[test]
fn sidecar_param_type_roundtrips_all_variants() {
    for kind in [
        SidecarParamType::String,
        SidecarParamType::Integer,
        SidecarParamType::Number,
        SidecarParamType::Boolean,
        SidecarParamType::Json,
    ] {
        let json = roundtrip(&kind);
        let decoded: SidecarParamType = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, kind);
    }
}

#[test]
fn sidecar_param_option_roundtrip() {
    let opt = SidecarParamOption {
        value: serde_json::json!("us-east-1"),
        label: Some("US East (N. Virginia)".to_string()),
    };
    let json = roundtrip(&opt);
    let decoded: SidecarParamOption = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded.value, serde_json::json!("us-east-1"));
    assert_eq!(decoded.label.as_deref(), Some("US East (N. Virginia)"));
}

#[test]
fn sidecar_param_field_type_key_renamed_to_type_in_json() {
    let field = SidecarParamField {
        key: "region".to_string(),
        kind: SidecarParamType::String,
        required: true,
        label: Some("Region".to_string()),
        help: None,
        default: None,
        options: vec![],
    };
    let json = serde_json::to_string(&field).unwrap();
    assert!(
        json.contains(r#""type""#),
        "kind must serialize as 'type': {json}"
    );
    assert!(
        !json.contains(r#""kind""#),
        "raw 'kind' key must not appear: {json}"
    );
    let decoded: SidecarParamField = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded.kind, SidecarParamType::String);
    assert_eq!(decoded.key, "region");
    assert!(decoded.required);
}

#[test]
fn sidecar_param_field_with_options_roundtrip() {
    let field = SidecarParamField {
        key: "env".to_string(),
        kind: SidecarParamType::String,
        required: false,
        label: None,
        help: Some("Deployment environment".to_string()),
        default: Some(serde_json::json!("production")),
        options: vec![
            SidecarParamOption {
                value: serde_json::json!("production"),
                label: Some("Production".to_string()),
            },
            SidecarParamOption {
                value: serde_json::json!("staging"),
                label: None,
            },
        ],
    };
    let json = roundtrip(&field);
    let decoded: SidecarParamField = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded.options.len(), 2);
    assert_eq!(decoded.default, Some(serde_json::json!("production")));
}

#[test]
fn sidecar_params_schema_defaults_to_empty_fields() {
    let json = r#"{}"#;
    let decoded: SidecarParamsSchema = serde_json::from_str(json).unwrap();
    assert!(decoded.fields.is_empty());
}

#[test]
fn sidecar_params_schema_roundtrip() {
    let schema = SidecarParamsSchema {
        fields: vec![SidecarParamField {
            key: "timeout".to_string(),
            kind: SidecarParamType::Integer,
            required: false,
            label: None,
            help: None,
            default: Some(serde_json::json!(30)),
            options: vec![],
        }],
    };
    let json = roundtrip(&schema);
    let decoded: SidecarParamsSchema = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded.fields.len(), 1);
    assert_eq!(decoded.fields[0].kind, SidecarParamType::Integer);
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
