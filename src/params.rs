use std::path::Path;

use crate::config::RuntimeLimits;
use crate::error::{BrrmmmmError, BrrmmmmResult};

pub fn parse_env_vars(raw: &[String]) -> Vec<(String, String)> {
    raw.iter()
        .filter_map(|s| {
            s.split_once('=')
                .map(|(k, v)| (k.to_string(), v.to_string()))
        })
        .collect()
}

pub fn parse_params_bytes(
    params_json: Option<&str>,
    params_file: Option<&Path>,
    limits: &RuntimeLimits,
) -> BrrmmmmResult<Option<Vec<u8>>> {
    let raw = if let Some(raw) = params_json {
        Some(raw.as_bytes().to_vec())
    } else if let Some(path) = params_file {
        Some(std::fs::read(path).map_err(|error| {
            BrrmmmmError::ParamsInvalid(format!("read params file {}: {error}", path.display()))
        })?)
    } else {
        None
    };
    let Some(raw) = raw else {
        return Ok(None);
    };
    if raw.len() > limits.max_params_bytes {
        return Err(BrrmmmmError::budget(
            "params",
            raw.len(),
            limits.max_params_bytes,
        ));
    }

    let value: serde_json::Value = serde_json::from_slice(&raw).map_err(|error| {
        BrrmmmmError::ParamsInvalid(format!("params must be valid JSON: {error}"))
    })?;
    if !value.is_object() {
        return Err(BrrmmmmError::ParamsInvalid(
            "sidecar params must be a JSON object".to_string(),
        ));
    }
    let depth = json_depth(&value);
    if depth > limits.max_json_depth {
        return Err(BrrmmmmError::budget(
            "params json depth",
            depth,
            limits.max_json_depth,
        ));
    }

    serde_json::to_vec(&value)
        .map(Some)
        .map_err(|error| BrrmmmmError::ParamsInvalid(format!("serialize sidecar params: {error}")))
}

fn json_depth(value: &serde_json::Value) -> usize {
    match value {
        serde_json::Value::Array(values) => values
            .iter()
            .map(json_depth)
            .max()
            .unwrap_or(0)
            .saturating_add(1),
        serde_json::Value::Object(values) => values
            .values()
            .map(json_depth)
            .max()
            .unwrap_or(0)
            .saturating_add(1),
        _ => 1,
    }
}
