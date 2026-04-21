use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use brrmmmm::config::{Config, RuntimeAssurance, RuntimeLimits};
use brrmmmm::error::BrrmmmmError;
use serde::Deserialize;

use crate::cmd::params::{parse_env_vars, parse_params_bytes, parse_params_value};
use crate::mission_result::MissionRecorder;

pub(crate) const WORKING_DIR_CONFIG_NAME: &str = "brrmmmm.toml";

#[derive(Debug)]
pub(crate) struct LoadedWorkingDirConfig {
    mission: Option<LoadedMissionConfig>,
    runtime: Option<LoadedRuntimeOverrides>,
    assurance: Option<AssuranceOverrides>,
    limits: Option<LimitOverrides>,
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedRun {
    pub(crate) wasm_path: PathBuf,
    pub(crate) env_vars: Vec<(String, String)>,
    pub(crate) params_bytes: Option<Vec<u8>>,
    pub(crate) mission_recorder: Option<MissionRecorder>,
    pub(crate) log_channel: bool,
    pub(crate) events_mode: bool,
    pub(crate) override_retry_gate: bool,
}

#[derive(Debug)]
struct LoadedMissionConfig {
    wasm: Option<PathBuf>,
    result_path: Option<PathBuf>,
    env: BTreeMap<String, String>,
    events: Option<bool>,
    log_channel: Option<bool>,
    params_file: Option<PathBuf>,
    params: Option<serde_json::Value>,
}

#[derive(Debug)]
struct LoadedRuntimeOverrides {
    tui_path: Option<PathBuf>,
    ai_model: Option<String>,
    browser_headless: Option<bool>,
    attestation: Option<bool>,
    identity_dir: Option<PathBuf>,
    state_dir: Option<PathBuf>,
    anthropic_api_key: Option<String>,
}

#[derive(Debug)]
struct AssuranceOverrides {
    same_reason_retry_limit: Option<u32>,
    default_retry_after_ms: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
struct WorkingDirConfig {
    #[serde(default)]
    mission: Option<MissionConfig>,
    #[serde(default)]
    runtime: Option<RuntimeOverrides>,
    #[serde(default)]
    assurance: Option<AssuranceConfig>,
    #[serde(default)]
    limits: Option<LimitOverrides>,
}

#[derive(Debug, Default, Deserialize)]
struct MissionConfig {
    wasm: Option<PathBuf>,
    result_path: Option<PathBuf>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    events: Option<bool>,
    log_channel: Option<bool>,
    params_file: Option<PathBuf>,
    #[serde(default)]
    params: Option<toml::Table>,
}

#[derive(Debug, Default, Deserialize)]
struct RuntimeOverrides {
    tui_path: Option<PathBuf>,
    ai_model: Option<String>,
    browser_headless: Option<bool>,
    attestation: Option<bool>,
    identity_dir: Option<PathBuf>,
    state_dir: Option<PathBuf>,
    anthropic_api_key: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct AssuranceConfig {
    same_reason_retry_limit: Option<u32>,
    default_retry_after_ms: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct LimitOverrides {
    kv_max_key_bytes: Option<usize>,
    kv_max_value_bytes: Option<usize>,
    kv_max_total_bytes: Option<usize>,
    max_params_bytes: Option<usize>,
    max_json_depth: Option<usize>,
    max_host_payload_bytes: Option<usize>,
    max_artifact_bytes: Option<usize>,
    max_artifact_preview_chars: Option<usize>,
    max_http_response_bytes: Option<usize>,
    max_ai_response_bytes: Option<usize>,
}

pub(crate) fn load_from_cwd() -> Result<Option<LoadedWorkingDirConfig>> {
    let cwd = std::env::current_dir().map_err(|error| {
        BrrmmmmError::ConfigInvalid(format!("resolve current directory: {error}"))
    })?;
    let path = cwd.join(WORKING_DIR_CONFIG_NAME);
    if !path.exists() {
        return Ok(None);
    }

    let raw = std::fs::read_to_string(&path).map_err(|error| {
        BrrmmmmError::ConfigInvalid(format!("read {}: {error}", path.display()))
    })?;
    let config: WorkingDirConfig = toml::from_str(&raw).map_err(|error| {
        BrrmmmmError::ConfigInvalid(format!("parse {}: {error}", path.display()))
    })?;
    LoadedWorkingDirConfig::from_raw(path, config).map(Some)
}

impl LoadedWorkingDirConfig {
    fn from_raw(path: PathBuf, raw: WorkingDirConfig) -> Result<Self> {
        let dir = path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));

        let mission = raw
            .mission
            .map(|mission| normalize_mission(&dir, mission))
            .transpose()?;
        let runtime = raw
            .runtime
            .map(|runtime| normalize_runtime(&dir, runtime))
            .transpose()?;
        let assurance = raw
            .assurance
            .map(normalize_assurance)
            .transpose()?;

        if let Some(limits) = &raw.limits {
            validate_limits(limits)?;
        }

        Ok(Self {
            mission,
            runtime,
            assurance,
            limits: raw.limits,
        })
    }

    pub(crate) fn apply_runtime_overrides(&self, config: &mut Config) -> Result<()> {
        if let Some(runtime) = &self.runtime {
            if let Some(path) = &runtime.tui_path {
                config.tui_path = Some(path.display().to_string());
            }
            if let Some(ai_model) = &runtime.ai_model {
                config.ai_model = ai_model.clone();
            }
            if let Some(browser_headless) = runtime.browser_headless {
                config.browser_headless = browser_headless;
            }
            if let Some(attestation) = runtime.attestation {
                config.attestation_disabled = !attestation;
            }
            if let Some(identity_dir) = &runtime.identity_dir {
                config.identity_dir = identity_dir.clone();
            }
            if let Some(state_dir) = &runtime.state_dir {
                config.state_dir = state_dir.clone();
            }
            if let Some(api_key) = &runtime.anthropic_api_key {
                config.anthropic_api_key = Some(api_key.clone());
            }
        }

        if let Some(limits) = &self.limits {
            apply_limits(&mut config.limits, limits);
        }
        if let Some(assurance) = &self.assurance {
            apply_assurance(&mut config.assurance, assurance);
        }

        Ok(())
    }

    pub(crate) fn resolve_wasm_path(&self, explicit: Option<&Path>) -> Result<PathBuf> {
        explicit
            .map(Path::to_path_buf)
            .or_else(|| {
                self.mission
                    .as_ref()
                    .and_then(|mission| mission.wasm.clone())
            })
            .ok_or_else(|| {
                BrrmmmmError::ConfigInvalid(format!(
                    "WASM path is required; pass one explicitly or set mission.wasm in {}",
                    WORKING_DIR_CONFIG_NAME
                ))
                .into()
            })
    }

    pub(crate) fn resolve_run(
        &self,
        wasm_path: Option<&Path>,
        env: &[String],
        params_json: Option<&str>,
        params_file: Option<&Path>,
        result_path: Option<&Path>,
        events_override: Option<bool>,
        log_channel_override: Option<bool>,
        override_retry_gate: bool,
        limits: &RuntimeLimits,
    ) -> Result<ResolvedRun> {
        resolve_run_inner(
            Some(self),
            wasm_path,
            env,
            params_json,
            params_file,
            result_path,
            events_override,
            log_channel_override,
            override_retry_gate,
            limits,
        )
    }
}

pub(crate) fn resolve_run_without_config(
    wasm_path: Option<&Path>,
    env: &[String],
    params_json: Option<&str>,
    params_file: Option<&Path>,
    result_path: Option<&Path>,
    events_override: Option<bool>,
    log_channel_override: Option<bool>,
    override_retry_gate: bool,
    limits: &RuntimeLimits,
) -> Result<ResolvedRun> {
    resolve_run_inner(
        None,
        wasm_path,
        env,
        params_json,
        params_file,
        result_path,
        events_override,
        log_channel_override,
        override_retry_gate,
        limits,
    )
}

fn resolve_run_inner(
    loaded: Option<&LoadedWorkingDirConfig>,
    wasm_path: Option<&Path>,
    env: &[String],
    params_json: Option<&str>,
    params_file: Option<&Path>,
    result_path: Option<&Path>,
    events_override: Option<bool>,
    log_channel_override: Option<bool>,
    override_retry_gate: bool,
    limits: &RuntimeLimits,
) -> Result<ResolvedRun> {
    let mission = loaded.and_then(|loaded| loaded.mission.as_ref());
    let resolved_wasm_path = wasm_path
        .map(Path::to_path_buf)
        .or_else(|| mission.and_then(|mission| mission.wasm.clone()));
    let resolved_result_path = result_path
        .map(Path::to_path_buf)
        .or_else(|| mission.and_then(|mission| mission.result_path.clone()));
    let recorder = resolved_result_path
        .clone()
        .map(|path| MissionRecorder::new(path, resolved_wasm_path.as_deref()));

    let wasm_path = resolved_wasm_path.ok_or_else(|| {
        BrrmmmmError::ConfigInvalid(format!(
            "WASM path is required; pass one explicitly or set mission.wasm in {}",
            WORKING_DIR_CONFIG_NAME
        ))
    });
    let wasm_path = match wasm_path {
        Ok(path) => path,
        Err(error) => return Err(record_failure(recorder.as_ref(), error.into())),
    };

    let mut merged_env = mission
        .map(|mission| mission.env.clone())
        .unwrap_or_default();
    for (key, value) in parse_env_vars(env) {
        merged_env.insert(key, value);
    }
    let env_vars = merged_env.into_iter().collect();

    let params_bytes = match resolve_params_bytes(mission, params_json, params_file, limits) {
        Ok(params) => params,
        Err(error) => return Err(record_failure(recorder.as_ref(), error)),
    };

    Ok(ResolvedRun {
        wasm_path,
        env_vars,
        params_bytes,
        mission_recorder: recorder,
        log_channel: log_channel_override
            .or_else(|| mission.and_then(|mission| mission.log_channel))
            .unwrap_or(false),
        events_mode: events_override
            .or_else(|| mission.and_then(|mission| mission.events))
            .unwrap_or(false),
        override_retry_gate,
    })
}

fn resolve_params_bytes(
    mission: Option<&LoadedMissionConfig>,
    params_json: Option<&str>,
    params_file: Option<&Path>,
    limits: &RuntimeLimits,
) -> Result<Option<Vec<u8>>> {
    if params_json.is_some() || params_file.is_some() {
        return parse_params_bytes(params_json, params_file, limits).map_err(Into::into);
    }

    if let Some(mission) = mission {
        if let Some(path) = mission.params_file.as_deref() {
            return parse_params_bytes(None, Some(path), limits).map_err(Into::into);
        }
        if let Some(value) = mission.params.as_ref() {
            return parse_params_value(value, limits)
                .map(Some)
                .map_err(Into::into);
        }
    }

    Ok(None)
}

fn record_failure(recorder: Option<&MissionRecorder>, error: anyhow::Error) -> anyhow::Error {
    let Some(recorder) = recorder else {
        return error;
    };
    match recorder.write_runtime_error(&error) {
        Ok(()) => error,
        Err(write_error) => write_error.context(format!("original run error: {error:#}")),
    }
}

fn normalize_mission(dir: &Path, mission: MissionConfig) -> Result<LoadedMissionConfig> {
    if mission.params_file.is_some() && mission.params.is_some() {
        return Err(BrrmmmmError::ConfigInvalid(
            "mission.params_file and [mission.params] are mutually exclusive".to_string(),
        )
        .into());
    }

    Ok(LoadedMissionConfig {
        wasm: mission
            .wasm
            .map(|path| resolve_path(dir, path, "mission.wasm"))
            .transpose()?,
        result_path: mission
            .result_path
            .map(|path| resolve_path(dir, path, "mission.result_path"))
            .transpose()?,
        env: mission.env,
        events: mission.events,
        log_channel: mission.log_channel,
        params_file: mission
            .params_file
            .map(|path| resolve_path(dir, path, "mission.params_file"))
            .transpose()?,
        params: mission
            .params
            .map(serde_json::to_value)
            .transpose()
            .map_err(|error| {
                BrrmmmmError::ConfigInvalid(format!(
                    "serialize [mission.params] to JSON object: {error}"
                ))
            })?,
    })
}

fn normalize_runtime(dir: &Path, runtime: RuntimeOverrides) -> Result<LoadedRuntimeOverrides> {
    Ok(LoadedRuntimeOverrides {
        tui_path: runtime
            .tui_path
            .map(|path| resolve_path(dir, path, "runtime.tui_path"))
            .transpose()?,
        ai_model: runtime
            .ai_model
            .map(|value| non_empty_string(value, "runtime.ai_model"))
            .transpose()?,
        browser_headless: runtime.browser_headless,
        attestation: runtime.attestation,
        identity_dir: runtime
            .identity_dir
            .map(|path| resolve_path(dir, path, "runtime.identity_dir"))
            .transpose()?,
        state_dir: runtime
            .state_dir
            .map(|path| resolve_path(dir, path, "runtime.state_dir"))
            .transpose()?,
        anthropic_api_key: runtime
            .anthropic_api_key
            .map(|value| non_empty_string(value, "runtime.anthropic_api_key"))
            .transpose()?,
    })
}

fn normalize_assurance(assurance: AssuranceConfig) -> Result<AssuranceOverrides> {
    for (name, value) in [
        (
            "assurance.same_reason_retry_limit",
            assurance.same_reason_retry_limit.map(u64::from),
        ),
        (
            "assurance.default_retry_after_ms",
            assurance.default_retry_after_ms,
        ),
    ] {
        if value == Some(0) {
            return Err(
                BrrmmmmError::ConfigInvalid(format!("{name} must be greater than zero")).into(),
            );
        }
    }

    Ok(AssuranceOverrides {
        same_reason_retry_limit: assurance.same_reason_retry_limit,
        default_retry_after_ms: assurance.default_retry_after_ms,
    })
}

fn resolve_path(dir: &Path, path: PathBuf, field_name: &str) -> Result<PathBuf> {
    if path.as_os_str().is_empty() {
        return Err(BrrmmmmError::ConfigInvalid(format!("{field_name} must not be empty")).into());
    }
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(dir.join(path))
    }
}

fn non_empty_string(value: String, field_name: &str) -> Result<String> {
    if value.trim().is_empty() {
        Err(BrrmmmmError::ConfigInvalid(format!("{field_name} must not be empty")).into())
    } else {
        Ok(value)
    }
}

fn validate_limits(limits: &LimitOverrides) -> Result<()> {
    for (name, value) in [
        ("limits.kv_max_key_bytes", limits.kv_max_key_bytes),
        ("limits.kv_max_value_bytes", limits.kv_max_value_bytes),
        ("limits.kv_max_total_bytes", limits.kv_max_total_bytes),
        ("limits.max_params_bytes", limits.max_params_bytes),
        ("limits.max_json_depth", limits.max_json_depth),
        (
            "limits.max_host_payload_bytes",
            limits.max_host_payload_bytes,
        ),
        ("limits.max_artifact_bytes", limits.max_artifact_bytes),
        (
            "limits.max_artifact_preview_chars",
            limits.max_artifact_preview_chars,
        ),
        (
            "limits.max_http_response_bytes",
            limits.max_http_response_bytes,
        ),
        ("limits.max_ai_response_bytes", limits.max_ai_response_bytes),
    ] {
        if value == Some(0) {
            return Err(
                BrrmmmmError::ConfigInvalid(format!("{name} must be greater than zero")).into(),
            );
        }
    }
    Ok(())
}

fn apply_limits(target: &mut RuntimeLimits, overrides: &LimitOverrides) {
    if let Some(value) = overrides.kv_max_key_bytes {
        target.kv_max_key_bytes = value;
    }
    if let Some(value) = overrides.kv_max_value_bytes {
        target.kv_max_value_bytes = value;
    }
    if let Some(value) = overrides.kv_max_total_bytes {
        target.kv_max_total_bytes = value;
    }
    if let Some(value) = overrides.max_params_bytes {
        target.max_params_bytes = value;
    }
    if let Some(value) = overrides.max_json_depth {
        target.max_json_depth = value;
    }
    if let Some(value) = overrides.max_host_payload_bytes {
        target.max_host_payload_bytes = value;
    }
    if let Some(value) = overrides.max_artifact_bytes {
        target.max_artifact_bytes = value;
    }
    if let Some(value) = overrides.max_artifact_preview_chars {
        target.max_artifact_preview_chars = value;
    }
    if let Some(value) = overrides.max_http_response_bytes {
        target.max_http_response_bytes = value;
    }
    if let Some(value) = overrides.max_ai_response_bytes {
        target.max_ai_response_bytes = value;
    }
}

fn apply_assurance(target: &mut RuntimeAssurance, overrides: &AssuranceOverrides) {
    if let Some(value) = overrides.same_reason_retry_limit {
        target.same_reason_retry_limit = value;
    }
    if let Some(value) = overrides.default_retry_after_ms {
        target.default_retry_after_ms = value;
    }
}
