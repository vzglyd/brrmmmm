const DESCRIBE: &[u8] = include_bytes!("describe.json");

#[link(wasm_import_module = "brrmmmm_host")]
extern "C" {
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
    let _ = mission_outcome_report as unsafe extern "C" fn(i32, i32) -> i32;
    loop {
        std::hint::spin_loop();
    }
}
