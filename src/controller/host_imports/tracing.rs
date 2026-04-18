pub(super) fn register(
    linker: &mut wasmtime::Linker<wasmtime_wasi::preview1::WasiP1Ctx>,
) -> anyhow::Result<()> {
    linker.func_wrap(
        "vzglyd_host",
        "trace_span_start",
        |_caller: wasmtime::Caller<'_, wasmtime_wasi::preview1::WasiP1Ctx>,
         _ptr: i32,
         _len: i32|
         -> i32 { 1 },
    )?;

    linker.func_wrap(
        "vzglyd_host",
        "trace_span_end",
        |_caller: wasmtime::Caller<'_, wasmtime_wasi::preview1::WasiP1Ctx>,
         _span_id: i32,
         _ptr: i32,
         _len: i32|
         -> i32 { 0 },
    )?;

    linker.func_wrap(
        "vzglyd_host",
        "trace_event",
        |_caller: wasmtime::Caller<'_, wasmtime_wasi::preview1::WasiP1Ctx>,
         _ptr: i32,
         _len: i32|
         -> i32 { 0 },
    )?;

    Ok(())
}
