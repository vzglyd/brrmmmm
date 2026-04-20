use crate::abi::SidecarPhase;

pub type BrrmmmmResult<T> = Result<T, BrrmmmmError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCategory {
    StateCorruption,
    BudgetExceeded,
    InvalidTransition,
    ParamsInvalid,
    PersistenceFailure,
    IdentityFailure,
    ConfigInvalid,
    Timeout,
    RuntimeFailure,
}

impl ErrorCategory {
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
            Self::RuntimeFailure => "runtime_failure",
        }
    }

    pub fn exit_code(self) -> i32 {
        match self {
            Self::ConfigInvalid | Self::ParamsInvalid | Self::BudgetExceeded => 64,
            Self::StateCorruption => 65,
            Self::InvalidTransition | Self::RuntimeFailure => 70,
            Self::PersistenceFailure | Self::IdentityFailure => 74,
            Self::Timeout => 124,
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum BrrmmmmError {
    #[error("state corruption: {0}")]
    StateCorruption(String),
    #[error("{resource} budget exceeded: {actual} bytes exceeds limit of {limit} bytes")]
    BudgetExceeded {
        resource: &'static str,
        actual: usize,
        limit: usize,
    },
    #[error("invalid phase transition: {from:?} -> {to:?}")]
    InvalidTransition {
        from: SidecarPhase,
        to: SidecarPhase,
    },
    #[error("invalid params: {0}")]
    ParamsInvalid(String),
    #[error("persistence failure: {0}")]
    PersistenceFailure(String),
    #[error("identity failure: {0}")]
    IdentityFailure(String),
    #[error("invalid configuration: {0}")]
    ConfigInvalid(String),
    #[error("timeout: {0}")]
    Timeout(String),
    #[error("runtime failure: {0}")]
    RuntimeFailure(String),
}

impl BrrmmmmError {
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
            Self::RuntimeFailure(_) => ErrorCategory::RuntimeFailure,
        }
    }

    pub fn exit_code(&self) -> i32 {
        self.category().exit_code()
    }

    pub fn budget(resource: &'static str, actual: usize, limit: usize) -> Self {
        Self::BudgetExceeded {
            resource,
            actual,
            limit,
        }
    }
}
