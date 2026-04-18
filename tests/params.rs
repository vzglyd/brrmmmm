use brrmmmm::params::{parse_env_vars, parse_params_bytes};

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
    let result = parse_params_bytes(None, None).unwrap();
    assert!(result.is_none());
}

#[test]
fn parse_params_bytes_accepts_json_object() {
    let result = parse_params_bytes(Some(r#"{"key":"value"}"#), None).unwrap();
    assert!(result.is_some());
    let bytes = result.unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["key"], "value");
}

#[test]
fn parse_params_bytes_rejects_json_array() {
    let result = parse_params_bytes(Some("[1,2,3]"), None);
    assert!(result.is_err());
}

#[test]
fn parse_params_bytes_rejects_invalid_json() {
    let result = parse_params_bytes(Some("not json"), None);
    assert!(result.is_err());
}

#[test]
fn parse_params_bytes_reads_from_file() {
    let path =
        std::env::temp_dir().join(format!("brrmmmm_params_test_{}.json", std::process::id()));
    std::fs::write(&path, r#"{"from":"file"}"#).unwrap();
    let result = parse_params_bytes(None, path.to_str()).unwrap();
    std::fs::remove_file(&path).ok();
    assert!(result.is_some());
    let value: serde_json::Value = serde_json::from_slice(&result.unwrap()).unwrap();
    assert_eq!(value["from"], "file");
}

#[test]
fn parse_params_bytes_errors_on_missing_file() {
    let result = parse_params_bytes(None, Some("/tmp/brrmmmm_nonexistent_xyz_abc_123.json"));
    assert!(result.is_err());
}
