const DESCRIBE: &[u8] = include_bytes!("describe.json");
const PUBLISHED_KIND: &[u8] = b"published_output";
const PUBLISHED_PAYLOAD: &[u8] = &[0xff, 0x00, 0x7f, 0x80];

#[link(wasm_import_module = "brrmmmm_host")]
extern "C" {
    fn artifact_publish(kind_ptr: i32, kind_len: i32, data_ptr: i32, data_len: i32) -> i32;
    fn mission_outcome_report(ptr: i32, len: i32) -> i32;
}

#[no_mangle]
pub extern "C" fn brrmmmm_module_abi_version() -> u32 {
    3
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
    unsafe {
        artifact_publish(
            PUBLISHED_KIND.as_ptr() as i32,
            PUBLISHED_KIND.len() as i32,
            PUBLISHED_PAYLOAD.as_ptr() as i32,
            PUBLISHED_PAYLOAD.len() as i32,
        );
        let outcome = r#"{"status":"published","reason_code":"published_output","message":"fixture published binary output","primary_artifact_kind":"published_output"}"#;
        mission_outcome_report(outcome.as_ptr() as i32, outcome.len() as i32);
    }
}
