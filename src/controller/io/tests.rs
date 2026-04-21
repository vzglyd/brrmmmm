use super::*;

#[test]
fn phase_transition_allows_expected_lifecycle() {
    assert!(is_valid_phase_transition(
        &MissionPhase::Idle,
        &MissionPhase::Fetching
    ));
    assert!(is_valid_phase_transition(
        &MissionPhase::Fetching,
        &MissionPhase::Parsing
    ));
    assert!(is_valid_phase_transition(
        &MissionPhase::Parsing,
        &MissionPhase::Publishing
    ));
    assert!(is_valid_phase_transition(
        &MissionPhase::Publishing,
        &MissionPhase::Idle
    ));
}

#[test]
fn phase_transition_rejects_invalid_jump() {
    assert!(!is_valid_phase_transition(
        &MissionPhase::Idle,
        &MissionPhase::Parsing
    ));
    assert!(!is_valid_phase_transition(
        &MissionPhase::Parsing,
        &MissionPhase::Fetching
    ));
}

#[test]
fn backoff_fixed_interval_ignores_failure_count() {
    let s = PollStrategy::FixedInterval { interval_secs: 60 };
    assert_eq!(compute_strategy_backoff_ms(&s, 0), 60_000);
    assert_eq!(compute_strategy_backoff_ms(&s, 5), 60_000);
    assert_eq!(compute_strategy_backoff_ms(&s, 100), 60_000);
}

#[test]
fn backoff_exponential_doubles_per_failure() {
    let s = PollStrategy::ExponentialBackoff {
        base_secs: 30,
        max_secs: 300,
    };
    assert_eq!(compute_strategy_backoff_ms(&s, 1), 30_000);
    assert_eq!(compute_strategy_backoff_ms(&s, 2), 60_000);
    assert_eq!(compute_strategy_backoff_ms(&s, 3), 120_000);
    assert_eq!(compute_strategy_backoff_ms(&s, 4), 240_000);
    assert_eq!(compute_strategy_backoff_ms(&s, 5), 300_000);
    assert_eq!(compute_strategy_backoff_ms(&s, 6), 300_000);
}

#[test]
fn backoff_exponential_clamps_on_overflow() {
    let s = PollStrategy::ExponentialBackoff {
        base_secs: 30,
        max_secs: 300,
    };
    assert_eq!(compute_strategy_backoff_ms(&s, u32::MAX), 300_000);
}

#[test]
fn backoff_jittered_returns_lower_bound() {
    let s = PollStrategy::Jittered {
        base_secs: 60,
        jitter_secs: 15,
    };
    assert_eq!(compute_strategy_backoff_ms(&s, 0), 45_000);
    assert_eq!(compute_strategy_backoff_ms(&s, 10), 45_000);
}

#[test]
fn backoff_jittered_saturates_when_jitter_exceeds_base() {
    let s = PollStrategy::Jittered {
        base_secs: 10,
        jitter_secs: 30,
    };
    assert_eq!(compute_strategy_backoff_ms(&s, 1), 0);
}

#[test]
fn acquisition_timeout_uses_assurance_default_over_poll_strategy() {
    let outcome = MissionOutcome {
        status: MissionOutcomeStatus::RetryableFailure,
        reason_code: "acquisition_timeout".to_string(),
        message: "timed out".to_string(),
        retry_after_ms: None,
        operator_action: None,
        operator_timeout_ms: None,
        operator_timeout_outcome: None,
        primary_artifact_kind: None,
    };
    let describe = MissionModuleDescribe {
        schema_version: 1,
        logical_id: "test.mission".to_string(),
        name: "Test".to_string(),
        description: "Test".to_string(),
        abi_version: 4,
        run_modes: vec!["managed_polling".to_string()],
        state_persistence: crate::abi::PersistenceAuthority::Volatile,
        required_env_vars: Vec::new(),
        optional_env_vars: Vec::new(),
        params: Some(crate::abi::MissionParamsSchema { fields: Vec::new() }),
        capabilities_needed: Vec::new(),
        poll_strategy: Some(PollStrategy::FixedInterval { interval_secs: 60 }),
        cooldown_policy: None,
        artifact_types: Vec::new(),
        acquisition_timeout_secs: Some(1),
        operator_fallback: None,
    };
    let assurance = RuntimeAssurance {
        same_reason_retry_limit: 3,
        default_retry_after_ms: 50,
    };

    assert_eq!(
        resolved_retry_after_ms(&outcome, &assurance, Some(&describe), 1),
        Some(50)
    );
}
