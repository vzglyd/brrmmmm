use std::ffi::OsString;
use std::path::PathBuf;

use crate::error::{BrrmmmmError, BrrmmmmResult};

#[derive(Clone, Debug)]
pub struct RuntimeLimits {
    pub kv_max_key_bytes: usize,
    pub kv_max_value_bytes: usize,
    pub kv_max_total_bytes: usize,
    pub max_params_bytes: usize,
    pub max_json_depth: usize,
    pub max_host_payload_bytes: usize,
    pub max_artifact_bytes: usize,
    pub max_artifact_preview_chars: usize,
    pub max_http_response_bytes: usize,
    pub max_ai_response_bytes: usize,
}

impl Default for RuntimeLimits {
    fn default() -> Self {
        Self {
            kv_max_key_bytes: 256,
            kv_max_value_bytes: 64 * 1024,
            kv_max_total_bytes: 1024 * 1024,
            max_params_bytes: 1024 * 1024,
            max_json_depth: 64,
            max_host_payload_bytes: 1024 * 1024,
            max_artifact_bytes: 10 * 1024 * 1024,
            max_artifact_preview_chars: 500,
            max_http_response_bytes: 10 * 1024 * 1024,
            max_ai_response_bytes: 10 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    pub tui_path: Option<String>,
    pub ai_model: String,
    pub browser_headless: bool,
    pub attestation_disabled: bool,
    pub identity_dir: PathBuf,
    pub state_dir: PathBuf,
    pub anthropic_api_key: Option<String>,
    pub limits: RuntimeLimits,
}

impl Config {
    pub fn load() -> BrrmmmmResult<Self> {
        let limits = RuntimeLimits::load()?;

        let attestation_disabled = match std::env::var("BRRMMMM_ATTESTATION") {
            Ok(value) => !parse_bool("BRRMMMM_ATTESTATION", &value)?,
            Err(std::env::VarError::NotPresent) => false,
            Err(error) => {
                return Err(BrrmmmmError::ConfigInvalid(format!(
                    "BRRMMMM_ATTESTATION is not valid UTF-8: {error}"
                )));
            }
        };

        let browser_headless = match std::env::var("BRRMMMM_BROWSER_HEADLESS") {
            Ok(value) => parse_bool("BRRMMMM_BROWSER_HEADLESS", &value)?,
            Err(std::env::VarError::NotPresent) => true,
            Err(error) => {
                return Err(BrrmmmmError::ConfigInvalid(format!(
                    "BRRMMMM_BROWSER_HEADLESS is not valid UTF-8: {error}"
                )));
            }
        };

        let ai_model = non_empty_env("BRRMMMM_AI_MODEL")?
            .unwrap_or_else(|| "claude-3-5-sonnet-20241022".to_string());
        let tui_path = non_empty_env("BRRMMMM_TUI")?;

        let identity_dir = normalize_path(
            env_path("BRRMMMM_IDENTITY_DIR")?.unwrap_or_else(default_identity_dir),
            "BRRMMMM_IDENTITY_DIR",
        )?;
        let state_dir = normalize_path(
            env_path("BRRMMMM_STATE_DIR")?.unwrap_or_else(default_state_dir),
            "BRRMMMM_STATE_DIR",
        )?;

        let anthropic_api_key = non_empty_env("ANTHROPIC_API_KEY")?;

        Ok(Self {
            tui_path,
            ai_model,
            browser_headless,
            attestation_disabled,
            identity_dir,
            state_dir,
            anthropic_api_key,
            limits,
        })
    }
}

impl RuntimeLimits {
    fn load() -> BrrmmmmResult<Self> {
        let defaults = Self::default();
        Ok(Self {
            kv_max_key_bytes: parse_limit_env(
                "BRRMMMM_KV_MAX_KEY_BYTES",
                defaults.kv_max_key_bytes,
            )?,
            kv_max_value_bytes: parse_limit_env(
                "BRRMMMM_KV_MAX_VALUE_BYTES",
                defaults.kv_max_value_bytes,
            )?,
            kv_max_total_bytes: parse_limit_env(
                "BRRMMMM_KV_MAX_TOTAL_BYTES",
                defaults.kv_max_total_bytes,
            )?,
            max_params_bytes: parse_limit_env(
                "BRRMMMM_MAX_PARAMS_BYTES",
                defaults.max_params_bytes,
            )?,
            max_json_depth: parse_limit_env("BRRMMMM_MAX_JSON_DEPTH", defaults.max_json_depth)?,
            max_host_payload_bytes: parse_limit_env(
                "BRRMMMM_MAX_HOST_PAYLOAD_BYTES",
                defaults.max_host_payload_bytes,
            )?,
            max_artifact_bytes: parse_limit_env(
                "BRRMMMM_MAX_ARTIFACT_BYTES",
                defaults.max_artifact_bytes,
            )?,
            max_artifact_preview_chars: parse_limit_env(
                "BRRMMMM_MAX_ARTIFACT_PREVIEW_CHARS",
                defaults.max_artifact_preview_chars,
            )?,
            max_http_response_bytes: parse_limit_env(
                "BRRMMMM_MAX_HTTP_RESPONSE_BYTES",
                defaults.max_http_response_bytes,
            )?,
            max_ai_response_bytes: parse_limit_env(
                "BRRMMMM_MAX_AI_RESPONSE_BYTES",
                defaults.max_ai_response_bytes,
            )?,
        })
    }
}

fn parse_bool(name: &str, value: &str) -> BrrmmmmResult<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" | "legacy" => Ok(false),
        _ => Err(BrrmmmmError::ConfigInvalid(format!(
            "{name} must be one of true/false, 1/0, yes/no, or on/off"
        ))),
    }
}

fn parse_limit_env(name: &str, default: usize) -> BrrmmmmResult<usize> {
    match std::env::var(name) {
        Ok(value) => {
            let value = value.trim();
            let parsed = value.parse::<usize>().map_err(|error| {
                BrrmmmmError::ConfigInvalid(format!("{name} must be a positive integer: {error}"))
            })?;
            if parsed == 0 {
                return Err(BrrmmmmError::ConfigInvalid(format!(
                    "{name} must be greater than zero"
                )));
            }
            Ok(parsed)
        }
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(error) => Err(BrrmmmmError::ConfigInvalid(format!(
            "{name} is not valid UTF-8: {error}"
        ))),
    }
}

fn non_empty_env(name: &str) -> BrrmmmmResult<Option<String>> {
    match std::env::var(name) {
        Ok(value) => {
            if value.trim().is_empty() {
                Err(BrrmmmmError::ConfigInvalid(format!(
                    "{name} must not be empty"
                )))
            } else {
                Ok(Some(value))
            }
        }
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(BrrmmmmError::ConfigInvalid(format!(
            "{name} is not valid UTF-8: {error}"
        ))),
    }
}

fn env_path(name: &str) -> BrrmmmmResult<Option<PathBuf>> {
    match std::env::var_os(name) {
        Some(value) => {
            if os_string_is_empty(&value) {
                Err(BrrmmmmError::ConfigInvalid(format!(
                    "{name} must not be empty"
                )))
            } else {
                Ok(Some(PathBuf::from(value)))
            }
        }
        None => Ok(None),
    }
}

fn os_string_is_empty(value: &OsString) -> bool {
    value.to_string_lossy().trim().is_empty()
}

fn normalize_path(path: PathBuf, name: &str) -> BrrmmmmResult<PathBuf> {
    if path.as_os_str().is_empty() {
        return Err(BrrmmmmError::ConfigInvalid(format!(
            "{name} must not be empty"
        )));
    }
    if path.is_absolute() {
        Ok(path)
    } else {
        let cwd = std::env::current_dir().map_err(|error| {
            BrrmmmmError::ConfigInvalid(format!("resolve current directory: {error}"))
        })?;
        Ok(cwd.join(path))
    }
}

fn default_identity_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(fallback_data_dir)
        .join("brrmmmm")
        .join("identity")
}

fn default_state_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(fallback_data_dir)
        .join("brrmmmm")
        .join("state")
}

fn fallback_data_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".local")
        .join("share")
}
