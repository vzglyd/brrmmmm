const DESCRIBE: &[u8] = include_bytes!("describe.json");
const PUBLISHED_KIND: &[u8] = b"published_output";

#[link(wasm_import_module = "brrmmmm_host")]
extern "C" {
    fn params_len() -> i32;
    fn params_read(ptr: i32, len: i32) -> i32;
    fn artifact_publish(kind_ptr: i32, kind_len: i32, data_ptr: i32, data_len: i32) -> i32;
    fn mission_outcome_report(ptr: i32, len: i32) -> i32;
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
    let len = unsafe { params_len() };
    if len < 0 {
        publish(br#"{"ok":false,"error":"params_len failed"}"#);
        report_outcome(
            "terminal_failure",
            "params_len_failed",
            "params_len failed",
        );
        return;
    }

    let mut params = vec![0u8; len as usize];
    let read = unsafe { params_read(params.as_mut_ptr() as i32, len) };
    if read != len {
        publish(br#"{"ok":false,"error":"params_read failed"}"#);
        report_outcome(
            "terminal_failure",
            "params_read_failed",
            "params_read failed",
        );
        return;
    }

    let mut payload = br#"{"ok":true,"params":"#.to_vec();
    payload.extend_from_slice(&params);
    payload.extend_from_slice(b"}");
    publish(&payload);
    report_outcome("published", "published_output", "fixture published params payload");
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
