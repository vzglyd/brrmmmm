const DESCRIBE: &[u8] = include_bytes!("describe.json");
const PUBLISHED_KIND: &[u8] = b"published_output";

#[link(wasm_import_module = "brrmmmm_host")]
extern "C" {
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
    let label = std::env::var("FIXTURE_LABEL").unwrap_or_default();
    let extra = std::env::var("EXTRA_LABEL").unwrap_or_default();
    let payload = format!(
        r#"{{"ok":true,"label":"{}","extra":"{}"}}"#,
        json_escape(&label),
        json_escape(&extra)
    );
    publish(payload.as_bytes());
    report_published();
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

fn report_published() {
    let outcome = r#"{"status":"published","reason_code":"published_output","message":"fixture published env payload","primary_artifact_kind":"published_output"}"#;
    unsafe {
        mission_outcome_report(outcome.as_ptr() as i32, outcome.len() as i32);
    }
}

fn json_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}
