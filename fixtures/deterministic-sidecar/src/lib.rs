const DESCRIBE: &[u8] = include_bytes!("describe.json");
const RAW_KIND: &[u8] = b"raw_source_payload";
const NORMALIZED_KIND: &[u8] = b"normalized_payload";
const PUBLISHED_KIND: &[u8] = b"published_output";
const RAW_PAYLOAD: &[u8] = br#"{"source":"fixture","items":[1,2,3]}"#;
const NORMALIZED_PAYLOAD: &[u8] = br#"{"items":[1,2,3],"count":3}"#;
const PUBLISHED_PAYLOAD: &[u8] =
    br#"{"ok":true,"source":"fixture","count":3,"items":[1,2,3]}"#;

#[link(wasm_import_module = "vzglyd_host")]
extern "C" {
    fn artifact_publish(kind_ptr: i32, kind_len: i32, data_ptr: i32, data_len: i32) -> i32;
}

#[no_mangle]
pub extern "C" fn vzglyd_sidecar_abi_version() -> u32 {
    1
}

#[no_mangle]
pub extern "C" fn vzglyd_sidecar_describe_ptr() -> i32 {
    DESCRIBE.as_ptr() as i32
}

#[no_mangle]
pub extern "C" fn vzglyd_sidecar_describe_len() -> i32 {
    DESCRIBE.len() as i32
}

#[no_mangle]
pub extern "C" fn vzglyd_sidecar_start() {
    publish(RAW_KIND, RAW_PAYLOAD);
    publish(NORMALIZED_KIND, NORMALIZED_PAYLOAD);
    publish(PUBLISHED_KIND, PUBLISHED_PAYLOAD);
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
