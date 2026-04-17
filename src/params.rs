use anyhow::{Context, Result};

pub fn parse_env_vars(raw: &[String]) -> Vec<(String, String)> {
    raw.iter()
        .filter_map(|s| s.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
        .collect()
}

pub fn parse_params_bytes(
    params_json: Option<&str>,
    params_file: Option<&str>,
) -> Result<Option<Vec<u8>>> {
    let raw = if let Some(raw) = params_json {
        Some(raw.to_string())
    } else if let Some(path) = params_file {
        Some(std::fs::read_to_string(path).with_context(|| format!("read params file: {path}"))?)
    } else {
        None
    };
    let Some(raw) = raw else {
        return Ok(None);
    };
    let value: serde_json::Value =
        serde_json::from_str(&raw).context("sidecar params must be valid JSON")?;
    if !value.is_object() {
        anyhow::bail!("sidecar params must be a JSON object");
    }
    serde_json::to_vec(&value).map(Some).context("serialize sidecar params")
}
