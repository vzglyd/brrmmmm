use std::sync::{Arc, Mutex};

use anyhow::Result;

use crate::abi::{PersistenceAuthority, SidecarRuntimeState};

const KV_MAX_VALUE_BYTES: usize = 64 * 1024;
const KV_MAX_TOTAL_BYTES: usize = 1024 * 1024;
use crate::events::{Event, EventSink, diag, now_ts};
use crate::host::HostState;
use crate::mission_state::{self, CAP_KV};
use crate::persistence;

use super::super::super::io::{WasmCaller, WasmLinker, lock_runtime, read_memory_from_caller};
use super::state::{clear_pending_kv_response, store_pending_kv_response};

type PersistFn = fn(&str, &SidecarRuntimeState) -> anyhow::Result<()>;

pub(super) fn register(
    linker: &mut WasmLinker,
    shared: Arc<Mutex<HostState>>,
    event_sink: EventSink,
    runtime_state: Arc<Mutex<SidecarRuntimeState>>,
    wasm_hash: Option<String>,
) -> Result<()> {
    // kv_get(key_ptr, key_len) -> i32
    {
        let shared = shared.clone();
        let event_sink = event_sink.clone();
        let runtime_state = runtime_state.clone();
        linker.func_wrap(
            "vzglyd_host",
            "kv_get",
            move |mut caller: WasmCaller<'_>, ptr: i32, len: i32| -> i32 {
                let key = match read_key(&mut caller, ptr, len, &event_sink, "kv_get") {
                    Ok(key) => key,
                    Err(status) => return status,
                };
                record_kv_activity(&shared, "get");

                let value = {
                    let state = lock_runtime(&runtime_state, "runtime_state");
                    state.kv.get(&key).cloned()
                };

                event_sink.emit(Event::KvGet {
                    ts: now_ts(),
                    key,
                    found: value.is_some(),
                });

                if let Some(data) = value {
                    store_pending_kv_response(&shared, data);
                    0
                } else {
                    clear_pending_kv_response(&shared);
                    -1
                }
            },
        )?;
    }

    // kv_set(key_ptr, key_len, value_ptr, value_len) -> i32
    {
        let shared = shared.clone();
        let event_sink = event_sink.clone();
        let runtime_state = runtime_state.clone();
        let wasm_hash = wasm_hash.clone();
        linker.func_wrap(
            "vzglyd_host",
            "kv_set",
            move |mut caller: WasmCaller<'_>,
                  key_ptr: i32,
                  key_len: i32,
                  val_ptr: i32,
                  val_len: i32|
                  -> i32 {
                let key = match read_key(&mut caller, key_ptr, key_len, &event_sink, "kv_set") {
                    Ok(key) => key,
                    Err(status) => return status,
                };
                record_kv_activity(&shared, "set");

                let val_bytes = match read_memory_from_caller(&mut caller, val_ptr, val_len) {
                    Ok(b) => b,
                    Err(e) => {
                        diag(
                            &event_sink,
                            &format!("[brrmmmm] kv_set value memory error: {e}"),
                        );
                        return -1;
                    }
                };

                event_sink.emit(Event::KvSet {
                    ts: now_ts(),
                    key: key.clone(),
                    value_len: val_bytes.len(),
                });

                clear_pending_kv_response(&shared);
                if let Err(error) = set_value(
                    &runtime_state,
                    key,
                    val_bytes,
                    wasm_hash.as_deref(),
                    persistence::save,
                ) {
                    diag(&event_sink, &format!("[brrmmmm] kv_set failed: {error}"));
                    return -1;
                }
                0
            },
        )?;
    }

    // kv_delete(key_ptr, key_len) -> i32
    {
        let shared = shared.clone();
        let event_sink = event_sink.clone();
        let runtime_state = runtime_state.clone();
        let wasm_hash = wasm_hash.clone();
        linker.func_wrap(
            "vzglyd_host",
            "kv_delete",
            move |mut caller: WasmCaller<'_>, ptr: i32, len: i32| -> i32 {
                let key = match read_key(&mut caller, ptr, len, &event_sink, "kv_delete") {
                    Ok(key) => key,
                    Err(status) => return status,
                };
                record_kv_activity(&shared, "delete");

                event_sink.emit(Event::KvDelete {
                    ts: now_ts(),
                    key: key.clone(),
                });

                clear_pending_kv_response(&shared);
                if let Err(error) = delete_value(
                    &runtime_state,
                    &key,
                    wasm_hash.as_deref(),
                    persistence::save,
                ) {
                    diag(&event_sink, &format!("[brrmmmm] kv_delete failed: {error}"));
                    return -1;
                }
                0
            },
        )?;
    }

    Ok(())
}

fn record_kv_activity(shared: &Arc<Mutex<HostState>>, operation: &str) {
    let event = mission_state::kv_event(operation);
    let mut host = lock_runtime(shared, "host_state");
    host.record_activity(CAP_KV, "kv", &event);
}

fn read_key(
    caller: &mut WasmCaller<'_>,
    ptr: i32,
    len: i32,
    event_sink: &EventSink,
    action: &str,
) -> std::result::Result<String, i32> {
    let key_bytes = match read_memory_from_caller(caller, ptr, len) {
        Ok(bytes) => bytes,
        Err(error) => {
            diag(
                event_sink,
                &format!("[brrmmmm] {action} key memory error: {error}"),
            );
            return Err(-1);
        }
    };
    String::from_utf8(key_bytes).map_err(|error| {
        diag(
            event_sink,
            &format!("[brrmmmm] {action} key is not valid UTF-8: {error}"),
        );
        -1
    })
}

fn set_value(
    runtime_state: &Arc<Mutex<SidecarRuntimeState>>,
    key: String,
    value: Vec<u8>,
    wasm_hash: Option<&str>,
    persist: PersistFn,
) -> std::result::Result<(), String> {
    if value.len() > KV_MAX_VALUE_BYTES {
        return Err(format!(
            "kv_set value too large: {} bytes (max {} bytes per key)",
            value.len(),
            KV_MAX_VALUE_BYTES
        ));
    }

    let mut state = lock_runtime(runtime_state, "runtime_state");

    let existing_total: usize = state
        .kv
        .iter()
        .filter(|(k, _)| k.as_str() != key.as_str())
        .map(|(_, v)| v.len())
        .sum();
    if existing_total + value.len() > KV_MAX_TOTAL_BYTES {
        return Err(format!(
            "kv_set would exceed total KV budget: {existing_total} + {} bytes (max {KV_MAX_TOTAL_BYTES} bytes total)",
            value.len()
        ));
    }

    let previous = state.kv.insert(key.clone(), value);
    if let Err(error) = persist_if_host_persisted(&state, wasm_hash, persist) {
        restore_value(&mut state, key, previous);
        return Err(error);
    }
    Ok(())
}

fn delete_value(
    runtime_state: &Arc<Mutex<SidecarRuntimeState>>,
    key: &str,
    wasm_hash: Option<&str>,
    persist: PersistFn,
) -> std::result::Result<(), String> {
    let mut state = lock_runtime(runtime_state, "runtime_state");
    let previous = state.kv.remove(key);
    if previous.is_none() {
        return Ok(());
    }
    if let Err(error) = persist_if_host_persisted(&state, wasm_hash, persist) {
        restore_value(&mut state, key.to_string(), previous);
        return Err(error);
    }
    Ok(())
}

fn restore_value(state: &mut SidecarRuntimeState, key: String, previous: Option<Vec<u8>>) {
    if let Some(previous) = previous {
        state.kv.insert(key, previous);
    } else {
        state.kv.remove(&key);
    }
}

fn persist_if_host_persisted(
    state: &SidecarRuntimeState,
    wasm_hash: Option<&str>,
    persist: PersistFn,
) -> std::result::Result<(), String> {
    let should_persist = state
        .describe
        .as_ref()
        .map(|describe| describe.state_persistence == PersistenceAuthority::HostPersisted)
        .unwrap_or(false);
    if !should_persist {
        return Ok(());
    }
    let wasm_hash =
        wasm_hash.ok_or_else(|| "host-persisted KV state has no WASM identity".to_string())?;
    persist(wasm_hash, state).map_err(|error| format!("{error:#}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::abi::SidecarDescribe;

    fn persisted_state() -> Arc<Mutex<SidecarRuntimeState>> {
        let mut state = SidecarRuntimeState::default();
        state.describe = Some(SidecarDescribe {
            schema_version: 1,
            logical_id: "brrmmmm.test.kv".to_string(),
            name: "KV Test".to_string(),
            description: "KV test".to_string(),
            abi_version: 1,
            run_modes: vec!["managed_polling".to_string()],
            state_persistence: PersistenceAuthority::HostPersisted,
            required_env_vars: vec![],
            optional_env_vars: vec![],
            params: None,
            capabilities_needed: vec!["kv".to_string()],
            poll_strategy: None,
            cooldown_policy: None,
            artifact_types: vec!["published_output".to_string()],
            acquisition_timeout_secs: None,
        });
        Arc::new(Mutex::new(state))
    }

    fn persist_ok(_wasm_hash: &str, _state: &SidecarRuntimeState) -> anyhow::Result<()> {
        Ok(())
    }

    fn persist_err(_wasm_hash: &str, _state: &SidecarRuntimeState) -> anyhow::Result<()> {
        anyhow::bail!("state directory is not writable")
    }

    #[test]
    fn set_value_rolls_back_new_key_when_persistence_fails() {
        let state = persisted_state();

        let result = set_value(
            &state,
            "token".to_string(),
            b"secret".to_vec(),
            Some("wasm-hash"),
            persist_err,
        );

        assert!(result.unwrap_err().contains("not writable"));
        assert!(
            !lock_runtime(&state, "runtime_state")
                .kv
                .contains_key("token")
        );
    }

    #[test]
    fn set_value_restores_previous_value_when_persistence_fails() {
        let state = persisted_state();
        lock_runtime(&state, "runtime_state")
            .kv
            .insert("token".to_string(), b"old".to_vec());

        let result = set_value(
            &state,
            "token".to_string(),
            b"new".to_vec(),
            Some("wasm-hash"),
            persist_err,
        );

        assert!(result.is_err());
        assert_eq!(
            lock_runtime(&state, "runtime_state").kv.get("token"),
            Some(&b"old".to_vec())
        );
    }

    #[test]
    fn delete_value_restores_deleted_value_when_persistence_fails() {
        let state = persisted_state();
        lock_runtime(&state, "runtime_state")
            .kv
            .insert("token".to_string(), b"secret".to_vec());

        let result = delete_value(&state, "token", Some("wasm-hash"), persist_err);

        assert!(result.is_err());
        assert_eq!(
            lock_runtime(&state, "runtime_state").kv.get("token"),
            Some(&b"secret".to_vec())
        );
    }

    #[test]
    fn delete_value_missing_key_does_not_require_persistence() {
        let state = persisted_state();

        let result = delete_value(&state, "missing", Some("wasm-hash"), persist_err);

        assert!(result.is_ok());
    }

    #[test]
    fn set_value_rejects_oversized_single_value() {
        let state = persisted_state();
        let oversized = vec![0u8; KV_MAX_VALUE_BYTES + 1];

        let result = set_value(
            &state,
            "big".to_string(),
            oversized,
            Some("wasm-hash"),
            persist_ok,
        );

        assert!(result.unwrap_err().contains("too large"));
    }

    #[test]
    fn set_value_rejects_when_total_budget_exceeded() {
        let state = persisted_state();
        // Fill the map: 16 keys × KV_MAX_VALUE_BYTES = KV_MAX_TOTAL_BYTES exactly.
        for i in 0..16usize {
            set_value(
                &state,
                format!("key{i}"),
                vec![0u8; KV_MAX_VALUE_BYTES],
                Some("wasm-hash"),
                persist_ok,
            )
            .unwrap_or_else(|e| panic!("key{i} should fit: {e}"));
        }

        // Any additional write (even 1 byte) must be rejected.
        let result = set_value(
            &state,
            "overflow".to_string(),
            vec![0u8; 1],
            Some("wasm-hash"),
            persist_ok,
        );

        assert!(result.unwrap_err().contains("total KV budget"));
    }

    #[test]
    fn set_value_replacing_existing_key_does_not_double_count_old_bytes() {
        let state = persisted_state();
        // Fill the map to capacity (16 × KV_MAX_VALUE_BYTES = KV_MAX_TOTAL_BYTES).
        for i in 0..16usize {
            set_value(
                &state,
                format!("key{i}"),
                vec![0u8; KV_MAX_VALUE_BYTES],
                Some("wasm-hash"),
                persist_ok,
            )
            .unwrap_or_else(|e| panic!("key{i} should fit: {e}"));
        }

        // Replace an existing key with a smaller value — this should not exceed the budget.
        let result = set_value(
            &state,
            "key0".to_string(),
            vec![1u8; 10],
            Some("wasm-hash"),
            persist_ok,
        );

        assert!(
            result.is_ok(),
            "replacing existing key should succeed: {result:?}"
        );
    }

    #[test]
    fn host_persisted_set_requires_wasm_identity() {
        let state = persisted_state();

        let result = set_value(
            &state,
            "token".to_string(),
            b"secret".to_vec(),
            None,
            persist_ok,
        );

        assert!(result.unwrap_err().contains("no WASM identity"));
        assert!(
            !lock_runtime(&state, "runtime_state")
                .kv
                .contains_key("token")
        );
    }
}
