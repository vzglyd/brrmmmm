const DESCRIBE: &[u8] = include_bytes!("describe.json");
const RAW_KIND: &[u8] = b"raw_source_payload";
const NORMALIZED_KIND: &[u8] = b"normalized_payload";
const PUBLISHED_KIND: &[u8] = b"published_output";
const RAW_PAYLOAD: &[u8] = br#"{"source":"fixture","items":[1,2,3]}"#;
const NORMALIZED_PAYLOAD: &[u8] = br#"{"items":[1,2,3],"count":3}"#;
const PUBLISHED_PAYLOAD: &[u8] =
    br#"{"ok":true,"source":"fixture","count":3,"items":[1,2,3]}"#;

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
    publish(RAW_KIND, RAW_PAYLOAD);
    publish(NORMALIZED_KIND, NORMALIZED_PAYLOAD);
    publish(PUBLISHED_KIND, PUBLISHED_PAYLOAD);
    report_published();
}

fn publish(kind: &[u8], data: &[u8]) {
    unsafe {
        artifact_publish(
            kind.as_ptr() as i32,
            kind.len() as i32,
            data.as_ptr() as i32,
            data.len() as i32,
        );
    }
}

fn report_published() {
    let outcome = r#"{"status":"published","reason_code":"published_output","message":"fixture published deterministic output","primary_artifact_kind":"published_output"}"#;
    unsafe {
        mission_outcome_report(outcome.as_ptr() as i32, outcome.len() as i32);
    }
}
