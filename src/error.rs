//! Structured runtime, configuration, and persistence errors for `brrmmmm`.

use crate::abi::MissionPhase;

/// Convenience alias for results that return [`BrrmmmmError`].
pub type BrrmmmmResult<T> = Result<T, BrrmmmmError>;

/// Stable coarse-grained error category used for logging and exit-code mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCategory {
    /// Persisted state existed but could not be decoded or trusted.
    StateCorruption,
    /// A configured byte or depth budget was exceeded.
    BudgetExceeded,
    /// A runtime phase transition violated controller invariants.
    InvalidTransition,
    /// CLI-provided params were malformed or outside configured limits.
    ParamsInvalid,
    /// Host-managed state could not be read, written, or synced.
    PersistenceFailure,
    /// Installation identity creation, loading, or validation failed.
    IdentityFailure,
    /// Environment-derived runtime configuration was invalid.
    ConfigInvalid,
    /// A sidecar mission exceeded its allowed time budget.
    Timeout,
    /// A mission attempt closed safely and should be retried later.
    RetryableFailure,
    /// A mission attempt is waiting for bounded operator rescue.
    OperatorActionRequired,
    /// A runtime failure occurred outside a narrower category.
    RuntimeFailure,
}

impl ErrorCategory {
    /// Return the stable snake_case string emitted in structured logs.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::StateCorruption => "state_corruption",
            Self::BudgetExceeded => "budget_exceeded",
            Self::InvalidTransition => "invalid_transition",
            Self::ParamsInvalid => "params_invalid",
            Self::PersistenceFailure => "persistence_failure",
            Self::IdentityFailure => "identity_failure",
            Self::ConfigInvalid => "config_invalid",
            Self::Timeout => "timeout",
            Self::RetryableFailure => "retryable_failure",
            Self::OperatorActionRequired => "operator_action_required",
            Self::RuntimeFailure => "runtime_failure",
        }
    }

    /// Return the process exit code associated with this category.
    pub fn exit_code(self) -> i32 {
        match self {
            Self::ConfigInvalid | Self::ParamsInvalid | Self::BudgetExceeded => 64,
            Self::StateCorruption | Self::OperatorActionRequired => 65,
            Self::RetryableFailure => 75,
            Self::InvalidTransition | Self::RuntimeFailure => 70,
            Self::PersistenceFailure | Self::IdentityFailure => 74,
            Self::Timeout => 124,
        }
    }
}

/// Concrete error type returned by runtime and configuration operations.
#[derive(thiserror::Error, Debug)]
pub enum BrrmmmmError {
    /// Persisted runtime state was present but malformed or otherwise inconsistent.
    #[error("state corruption: {0}")]
    StateCorruption(String),
    /// A configured resource budget was exceeded.
    #[error("{resource} budget exceeded: {actual} bytes exceeds limit of {limit} bytes")]
    BudgetExceeded {
        /// Resource name used in the error message.
        resource: &'static str,
        /// Observed size or depth.
        actual: usize,
        /// Configured limit that was exceeded.
        limit: usize,
    },
    /// The runtime attempted an invalid lifecycle transition.
    #[error("invalid phase transition: {from:?} -> {to:?}")]
    InvalidTransition {
        /// Source lifecycle phase.
        from: MissionPhase,
        /// Target lifecycle phase.
        to: MissionPhase,
    },
    /// CLI params or sidecar-supplied params were invalid.
    #[error("invalid params: {0}")]
    ParamsInvalid(String),
    /// Host-managed persistence work failed.
    #[error("persistence failure: {0}")]
    PersistenceFailure(String),
    /// Installation identity work failed.
    #[error("identity failure: {0}")]
    IdentityFailure(String),
    /// Environment-derived runtime configuration was invalid.
    #[error("invalid configuration: {0}")]
    ConfigInvalid(String),
    /// A time budget or wait deadline was exceeded.
    #[error("timeout: {0}")]
    Timeout(String),
    /// The mission attempt closed safely and should be retried later.
    #[error("retryable failure: {0}")]
    RetryableFailure(String),
    /// The mission attempt is waiting for bounded operator rescue.
    #[error("operator action required: {0}")]
    OperatorActionRequired(String),
    /// An uncategorized runtime failure occurred.
    #[error("runtime failure: {0}")]
    RuntimeFailure(String),
}

impl BrrmmmmError {
    /// Return the coarse-grained category for this error.
    pub fn category(&self) -> ErrorCategory {
        match self {
            Self::StateCorruption(_) => ErrorCategory::StateCorruption,
            Self::BudgetExceeded { .. } => ErrorCategory::BudgetExceeded,
            Self::InvalidTransition { .. } => ErrorCategory::InvalidTransition,
            Self::ParamsInvalid(_) => ErrorCategory::ParamsInvalid,
            Self::PersistenceFailure(_) => ErrorCategory::PersistenceFailure,
            Self::IdentityFailure(_) => ErrorCategory::IdentityFailure,
            Self::ConfigInvalid(_) => ErrorCategory::ConfigInvalid,
            Self::Timeout(_) => ErrorCategory::Timeout,
            Self::RetryableFailure(_) => ErrorCategory::RetryableFailure,
            Self::OperatorActionRequired(_) => ErrorCategory::OperatorActionRequired,
            Self::RuntimeFailure(_) => ErrorCategory::RuntimeFailure,
        }
    }

    /// Return the process exit code associated with this error.
    pub fn exit_code(&self) -> i32 {
        self.category().exit_code()
    }

    /// Construct a standardized budget-exceeded error.
    pub fn budget(resource: &'static str, actual: usize, limit: usize) -> Self {
        Self::BudgetExceeded {
            resource,
            actual,
            limit,
        }
    }
}
