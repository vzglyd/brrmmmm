const DESCRIBE: &[u8] = include_bytes!("describe.json");
const SESSION_KEY: &[u8] = b"session_id";
const SESSION_VALUE: &[u8] = b"abc-123";
const PERSISTED_KEY: &[u8] = b"persisted_token";
const PERSISTED_VALUE: &[u8] = b"secret-token";
const PUBLISHED_KIND: &[u8] = b"published_output";

#[link(wasm_import_module = "brrmmmm_host")]
extern "C" {
    fn kv_get(key_ptr: i32, key_len: i32) -> i32;
    fn kv_set(key_ptr: i32, key_len: i32, value_ptr: i32, value_len: i32) -> i32;
    fn kv_delete(key_ptr: i32, key_len: i32) -> i32;
    fn kv_response_len() -> i32;
    fn kv_response_read(ptr: i32, len: i32) -> i32;
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
    if let Err(error) = run() {
        publish_json(&format!(r#"{{"ok":false,"error":"{error}"}}"#));
        report_outcome("terminal_failure", "kv_roundtrip_failed", error);
        return;
    }
    report_outcome("published", "published_output", "fixture published KV payload");
}

fn run() -> Result<(), &'static str> {
    set(SESSION_KEY, SESSION_VALUE)?;
    let loaded = get(SESSION_KEY)?;
    if loaded != SESSION_VALUE {
        return Err("session value mismatch");
    }

    delete(SESSION_KEY)?;
    let missing_status = unsafe { kv_get(SESSION_KEY.as_ptr() as i32, SESSION_KEY.len() as i32) };
    if missing_status != -1 {
        return Err("deleted key was still readable");
    }
    if unsafe { kv_response_len() } != -1 {
        return Err("missing key left a pending response");
    }

    set(PERSISTED_KEY, PERSISTED_VALUE)?;
    publish_json(r#"{"ok":true,"roundtrip":"abc-123","deleted_missing":true}"#);
    Ok(())
}

fn set(key: &[u8], value: &[u8]) -> Result<(), &'static str> {
    let status = unsafe {
        kv_set(
            key.as_ptr() as i32,
            key.len() as i32,
            value.as_ptr() as i32,
            value.len() as i32,
        )
    };
    if status == 0 {
        Ok(())
    } else {
        Err("kv_set failed")
    }
}

fn get(key: &[u8]) -> Result<Vec<u8>, &'static str> {
    let status = unsafe { kv_get(key.as_ptr() as i32, key.len() as i32) };
    if status != 0 {
        return Err("kv_get failed");
    }
    let len = unsafe { kv_response_len() };
    if len < 0 {
        return Err("kv_response_len failed");
    }
    let mut buf = vec![0u8; len as usize];
    let read = unsafe { kv_response_read(buf.as_mut_ptr() as i32, len) };
    if read != len {
        return Err("kv_response_read failed");
    }
    Ok(buf)
}

fn delete(key: &[u8]) -> Result<(), &'static str> {
    let status = unsafe { kv_delete(key.as_ptr() as i32, key.len() as i32) };
    if status == 0 {
        Ok(())
    } else {
        Err("kv_delete failed")
    }
}

fn publish_json(json: &str) {
    unsafe {
        artifact_publish(
            PUBLISHED_KIND.as_ptr() as i32,
            PUBLISHED_KIND.len() as i32,
            json.as_ptr() as i32,
            json.len() as i32,
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
