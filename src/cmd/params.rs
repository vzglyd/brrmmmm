use std::path::Path;

use brrmmmm::config::RuntimeLimits;
use brrmmmm::error::{BrrmmmmError, BrrmmmmResult};

pub(crate) fn parse_env_vars(raw: &[String]) -> Vec<(String, String)> {
    raw.iter()
        .filter_map(|value| {
            value
                .split_once('=')
                .map(|(key, value)| (key.to_string(), value.to_string()))
        })
        .collect()
}

pub(crate) fn parse_params_bytes(
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
    validate_params_value(&value, limits)?;

    serde_json::to_vec(&value)
        .map(Some)
        .map_err(|error| BrrmmmmError::ParamsInvalid(format!("serialize sidecar params: {error}")))
}

pub(crate) fn parse_params_value(
    value: &serde_json::Value,
    limits: &RuntimeLimits,
) -> BrrmmmmResult<Vec<u8>> {
    validate_params_value(value, limits)?;
    let bytes = serde_json::to_vec(value).map_err(|error| {
        BrrmmmmError::ParamsInvalid(format!("serialize sidecar params: {error}"))
    })?;
    if bytes.len() > limits.max_params_bytes {
        return Err(BrrmmmmError::budget(
            "params",
            bytes.len(),
            limits.max_params_bytes,
        ));
    }
    Ok(bytes)
}

fn validate_params_value(value: &serde_json::Value, limits: &RuntimeLimits) -> BrrmmmmResult<()> {
    if !value.is_object() {
        return Err(BrrmmmmError::ParamsInvalid(
            "sidecar params must be a JSON object".to_string(),
        ));
    }
    let depth = json_depth(value);
    if depth > limits.max_json_depth {
        return Err(BrrmmmmError::budget(
            "params json depth",
            depth,
            limits.max_json_depth,
        ));
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> RuntimeLimits {
        RuntimeLimits::default()
    }

    #[test]
    fn parse_env_vars_splits_on_first_equals() {
        let input = ["KEY=VALUE".to_string()];
        let result = parse_env_vars(&input);
        assert_eq!(result, [("KEY".to_string(), "VALUE".to_string())]);
    }

    #[test]
    fn parse_env_vars_handles_value_containing_equals() {
        let input = ["KEY=a=b".to_string()];
        let result = parse_env_vars(&input);
        assert_eq!(result, [("KEY".to_string(), "a=b".to_string())]);
    }

    #[test]
    fn parse_env_vars_ignores_entries_without_equals() {
        let input = ["NOEQUALS".to_string()];
        let result = parse_env_vars(&input);
        assert!(result.is_empty());
    }

    #[test]
    fn parse_env_vars_empty_input() {
        let result = parse_env_vars(&[]);
        assert!(result.is_empty());
    }

    #[test]
    fn parse_env_vars_multiple_pairs() {
        let input = ["A=1".to_string(), "B=2".to_string(), "C=three".to_string()];
        let result = parse_env_vars(&input);
        assert_eq!(result.len(), 3);
        assert_eq!(result[0], ("A".to_string(), "1".to_string()));
        assert_eq!(result[1], ("B".to_string(), "2".to_string()));
        assert_eq!(result[2], ("C".to_string(), "three".to_string()));
    }

    #[test]
    fn parse_params_bytes_returns_none_when_both_absent() {
        let result = parse_params_bytes(None, None, &limits()).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn parse_params_bytes_accepts_json_object() {
        let result = parse_params_bytes(Some(r#"{"key":"value"}"#), None, &limits()).unwrap();
        assert!(result.is_some());
        let bytes = result.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["key"], "value");
    }

    #[test]
    fn parse_params_bytes_rejects_json_array() {
        let result = parse_params_bytes(Some("[1,2,3]"), None, &limits());
        assert!(result.is_err());
    }

    #[test]
    fn parse_params_bytes_rejects_invalid_json() {
        let result = parse_params_bytes(Some("not json"), None, &limits());
        assert!(result.is_err());
    }

    #[test]
    fn parse_params_bytes_rejects_oversized_json() {
        let limits = RuntimeLimits {
            max_params_bytes: 8,
            ..RuntimeLimits::default()
        };
        let result = parse_params_bytes(Some(r#"{"key":"value"}"#), None, &limits);
        assert!(result.is_err());
    }

    #[test]
    fn parse_params_bytes_rejects_excessive_depth() {
        let limits = RuntimeLimits {
            max_json_depth: 2,
            ..RuntimeLimits::default()
        };
        let result = parse_params_bytes(Some(r#"{"a":{"b":1}}"#), None, &limits);
        assert!(result.is_err());
    }

    #[test]
    fn parse_params_bytes_reads_from_file() {
        let path =
            std::env::temp_dir().join(format!("brrmmmm_params_test_{}.json", std::process::id()));
        std::fs::write(&path, r#"{"from":"file"}"#).unwrap();
        let result = parse_params_bytes(None, Some(path.as_path()), &limits()).unwrap();
        std::fs::remove_file(&path).ok();
        assert!(result.is_some());
        let value: serde_json::Value = serde_json::from_slice(&result.unwrap()).unwrap();
        assert_eq!(value["from"], "file");
    }

    #[test]
    fn parse_params_bytes_errors_on_missing_file() {
        let result = parse_params_bytes(
            None,
            Some(Path::new("/tmp/brrmmmm_nonexistent_xyz_abc_123.json")),
            &limits(),
        );
        assert!(result.is_err());
    }
}
