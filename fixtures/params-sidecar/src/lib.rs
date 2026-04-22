const DESCRIBE: &[u8] = include_bytes!("describe.json");
const PUBLISHED_KIND: &[u8] = b"published_output";

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[link(wasm_import_module = "brrmmmm_host")]
extern "C" {
    fn params_len() -> i32;
    fn params_read(ptr: i32, len: i32) -> i32;
    fn artifact_publish(kind_ptr: i32, kind_len: i32, data_ptr: i32, data_len: i32) -> i32;
    fn mission_outcome_report(ptr: i32, len: i32) -> i32;
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
struct FixtureParams {
    label: String,
    repeat: i32,
    urgent: bool,
    mode: String,
    payload: Value,
}

impl Default for FixtureParams {
    fn default() -> Self {
        Self {
            label: "demo".to_string(),
            repeat: 3,
            urgent: false,
            mode: "summary".to_string(),
            payload: json!({
                "source": "params-sidecar",
                "tags": ["demo", "fixture"]
            }),
        }
    }
}

#[no_mangle]
pub extern "C" fn brrmmmm_module_abi_version() -> u32 {
    4
}

#[no_mangle]
pub extern "C" fn brrmmmm_module_describe_ptr() -> i32 {
    DESCRIBE.as_ptr() as i32
}

#[no_mangle]
pub extern "C" fn brrmmmm_module_describe_len() -> i32 {
    DESCRIBE.len() as i32
}

#[no_mangle]
pub extern "C" fn brrmmmm_module_start() {
    let params = match read_params() {
        Ok(params) => params,
        Err(error) => {
            let payload = json!({
                "ok": false,
                "error": error
            });
            let bytes = payload.to_string();
            publish(bytes.as_bytes());
            report_outcome("terminal_failure", "params_read_failed", &error);
            return;
        }
    };

    let payload = json!({
        "ok": true,
        "label": params.label,
        "repeat": params.repeat,
        "urgent": params.urgent,
        "mode": params.mode,
        "payload": params.payload,
    });
    let bytes = payload.to_string();
    publish(bytes.as_bytes());
    report_outcome("published", "published_output", "fixture published params payload");
}

fn read_params() -> Result<FixtureParams, String> {
    let len = unsafe { params_len() };
    if len < 0 {
        return Err("params_len failed".to_string());
    }
    if len == 0 {
        return Ok(FixtureParams::default());
    }

    let mut params = vec![0u8; len as usize];
    let read = unsafe { params_read(params.as_mut_ptr() as i32, len) };
    if read != len {
        return Err("params_read failed".to_string());
    }

    serde_json::from_slice(&params).map_err(|error| format!("invalid params JSON: {error}"))
}

fn publish(data: &[u8]) {
    unsafe {
        artifact_publish(
            PUBLISHED_KIND.as_ptr() as i32,
            PUBLISHED_KIND.len() as i32,
            data.as_ptr() as i32,
            data.len() as i32,
        );
    }
}

fn report_outcome(status: &str, reason_code: &str, message: &str) {
    let outcome = format!(
        r#"{{"status":"{status}","reason_code":"{reason_code}","message":"{message}","primary_artifact_kind":"published_output"}}"#
    );
    unsafe {
        mission_outcome_report(outcome.as_ptr() as i32, outcome.len() as i32);
    }
}
