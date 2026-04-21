use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};

use crate::abi::MissionRuntimeState;
use crate::events::{Event, EventSink, ms_to_iso8601, now_ms, now_ts};

use super::super::io::{WasmCaller, WasmLinker, update_sleep_state};

pub(super) fn register(
    linker: &mut WasmLinker,
    event_sink: EventSink,
    runtime_state: Arc<Mutex<MissionRuntimeState>>,
    stop_signal: Arc<AtomicBool>,
    force_refresh: Arc<AtomicBool>,
) -> anyhow::Result<()> {
    let sink_host_sleep = event_sink.clone();
    let force_refresh_host_sleep = force_refresh.clone();
    let stop_host_sleep = stop_signal;
    let runtime_host_sleep = runtime_state.clone();
    linker.func_wrap(
        "brrmmmm_host",
        "sleep_ms",
        move |_caller: WasmCaller<'_>, duration_ms: i64| -> i32 {
            if duration_ms <= 0 {
                return 0;
            }
            let duration_ms = duration_ms as u64;
            let wake_ms = now_ms().saturating_add(duration_ms);
            update_sleep_state(&runtime_host_sleep, &sink_host_sleep, duration_ms, wake_ms);
            sink_host_sleep.emit(Event::SleepStart {
                ts: now_ts(),
                duration_ms: duration_ms as i64,
                wake_at: ms_to_iso8601(wake_ms),
            });

            let started = Instant::now();
            let total = Duration::from_millis(duration_ms);
            loop {
                if stop_host_sleep.load(Ordering::Relaxed) {
                    return 1;
                }
                if force_refresh_host_sleep.swap(false, Ordering::Relaxed) {
                    return 1;
                }
                let elapsed = started.elapsed();
                if elapsed >= total {
                    return 0;
                }
                let remaining = total.saturating_sub(elapsed);
                thread::sleep(remaining.min(Duration::from_millis(100)));
            }
        },
    )?;

    let sink_sleep = event_sink;
    let force_refresh_sleep = force_refresh;
    let runtime_sleep = runtime_state;
    linker.func_wrap(
        "brrmmmm_host",
        "announce_sleep",
        move |_caller: WasmCaller<'_>, duration_ms: i64| -> i32 {
            if force_refresh_sleep.swap(false, Ordering::Relaxed) {
                return 1;
            }
            let wake_ms = now_ms().saturating_add(duration_ms.unsigned_abs());
            let wake_at = ms_to_iso8601(wake_ms);
            update_sleep_state(
                &runtime_sleep,
                &sink_sleep,
                duration_ms.unsigned_abs(),
                wake_ms,
            );
            sink_sleep.emit(Event::SleepStart {
                ts: now_ts(),
                duration_ms,
                wake_at,
            });
            0
        },
    )?;

    Ok(())
}
