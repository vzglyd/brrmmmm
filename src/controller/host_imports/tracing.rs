use super::super::io::{WasmCaller, WasmLinker};

pub(super) fn register(linker: &mut WasmLinker) -> anyhow::Result<()> {
    linker.func_wrap(
        "brrmmmm_host",
        "trace_span_start",
        |_caller: WasmCaller<'_>, _ptr: i32, _len: i32| -> i32 { 1 },
    )?;

    linker.func_wrap(
        "brrmmmm_host",
        "trace_span_end",
        |_caller: WasmCaller<'_>, _span_id: i32, _ptr: i32, _len: i32| -> i32 { 0 },
    )?;

    linker.func_wrap(
        "brrmmmm_host",
        "trace_event",
        |_caller: WasmCaller<'_>, _ptr: i32, _len: i32| -> i32 { 0 },
    )?;

    Ok(())
}
