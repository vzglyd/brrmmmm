use std::sync::{Arc, Mutex};

use anyhow::Result;

use crate::abi::{MissionRuntimeState, PersistenceAuthority};
use crate::config::Config;
use crate::error::BrrmmmmResult;

use crate::events::{Event, EventSink, diag, now_ts};
use crate::host::HostState;
use crate::mission_state::{self, Capabilities};
use crate::persistence;

use super::super::super::io::{WasmCaller, WasmLinker, lock_runtime, read_memory_from_caller};
use super::state::{clear_pending_kv_response, store_pending_kv_response};

type PersistFn = fn(&Config, &str, &MissionRuntimeState) -> BrrmmmmResult<()>;

pub(super) fn register(
    linker: &mut WasmLinker,
    shared: Arc<Mutex<HostState>>,
    event_sink: EventSink,
    runtime_state: Arc<Mutex<MissionRuntimeState>>,
    wasm_hash: Option<String>,
) -> Result<()> {
    register_get(
        linker,
        shared.clone(),
        event_sink.clone(),
        runtime_state.clone(),
    )?;
    register_set(
        linker,
        shared.clone(),
        event_sink.clone(),
        runtime_state.clone(),
        wasm_hash.clone(),
    )?;
    register_delete(linker, shared, event_sink, runtime_state, wasm_hash)?;
    Ok(())
}

fn register_get(
    linker: &mut WasmLinker,
    shared: Arc<Mutex<HostState>>,
    event_sink: EventSink,
    runtime_state: Arc<Mutex<MissionRuntimeState>>,
) -> Result<()> {
    linker.func_wrap(
        "brrmmmm_host",
        "kv_get",
        move |mut caller: WasmCaller<'_>, ptr: i32, len: i32| -> i32 {
            let limits = lock_runtime(&shared, "host_state").config.limits.clone();
            let key = match read_key(
                &mut caller,
                ptr,
                len,
                limits.kv_max_key_bytes,
                &event_sink,
                "kv_get",
            ) {
                Ok(key) => key,
                Err(status) => return status,
            };
            record_kv_activity(&shared, "get");

            let value = {
                let state = lock_runtime(&runtime_state, "runtime_state");
                state.kv.get(&key).cloned()
            };

            event_sink.emit(&Event::KvGet {
                ts: now_ts(),
                key,
                found: value.is_some(),
            });

            value.map_or_else(
                || {
                    clear_pending_kv_response(&shared);
                    -1
                },
                |data| {
                    store_pending_kv_response(&shared, data);
                    0
                },
            )
        },
    )?;
    Ok(())
}

fn register_set(
    linker: &mut WasmLinker,
    shared: Arc<Mutex<HostState>>,
    event_sink: EventSink,
    runtime_state: Arc<Mutex<MissionRuntimeState>>,
    wasm_hash: Option<String>,
) -> Result<()> {
    linker.func_wrap(
        "brrmmmm_host",
        "kv_set",
        move |mut caller: WasmCaller<'_>,
              key_ptr: i32,
              key_len: i32,
              val_ptr: i32,
              val_len: i32|
              -> i32 {
            let limits = lock_runtime(&shared, "host_state").config.limits.clone();
            let key = match read_key(
                &mut caller,
                key_ptr,
                key_len,
                limits.kv_max_key_bytes,
                &event_sink,
                "kv_set",
            ) {
                Ok(key) => key,
                Err(status) => return status,
            };
            record_kv_activity(&shared, "set");

            let Some(value_len) = usize::try_from(val_len).ok() else {
                return invalid_value_len(&event_sink, val_len, limits.kv_max_value_bytes);
            };
            if value_len > limits.kv_max_value_bytes {
                return invalid_value_len(&event_sink, val_len, limits.kv_max_value_bytes);
            }

            let val_bytes = match read_memory_from_caller(&mut caller, val_ptr, val_len) {
                Ok(bytes) => bytes,
                Err(error) => {
                    diag(
                        &event_sink,
                        &format!("[brrmmmm] kv_set value memory error: {error}"),
                    );
                    return -1;
                }
            };

            event_sink.emit(&Event::KvSet {
                ts: now_ts(),
                key: key.clone(),
                value_len,
            });

            clear_pending_kv_response(&shared);
            let config = lock_runtime(&shared, "host_state").config.clone();
            if let Err(error) = set_value(
                &config,
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
    Ok(())
}

fn register_delete(
    linker: &mut WasmLinker,
    shared: Arc<Mutex<HostState>>,
    event_sink: EventSink,
    runtime_state: Arc<Mutex<MissionRuntimeState>>,
    wasm_hash: Option<String>,
) -> Result<()> {
    linker.func_wrap(
        "brrmmmm_host",
        "kv_delete",
        move |mut caller: WasmCaller<'_>, ptr: i32, len: i32| -> i32 {
            let limits = lock_runtime(&shared, "host_state").config.limits.clone();
            let key = match read_key(
                &mut caller,
                ptr,
                len,
                limits.kv_max_key_bytes,
                &event_sink,
                "kv_delete",
            ) {
                Ok(key) => key,
                Err(status) => return status,
            };
            record_kv_activity(&shared, "delete");

            event_sink.emit(&Event::KvDelete {
                ts: now_ts(),
                key: key.clone(),
            });

            clear_pending_kv_response(&shared);
            let config = lock_runtime(&shared, "host_state").config.clone();
            if let Err(error) = delete_value(
                &config,
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
    Ok(())
}

fn record_kv_activity(shared: &Arc<Mutex<HostState>>, operation: &str) {
    let event = mission_state::kv_event(operation);
    let mut host = lock_runtime(shared, "host_state");
    host.record_activity(Capabilities::KV, "kv", &event);
}

fn read_key(
    caller: &mut WasmCaller<'_>,
    ptr: i32,
    len_i32: i32,
    max_key_bytes: usize,
    event_sink: &EventSink,
    action: &str,
) -> std::result::Result<String, i32> {
    let Some(len) = usize::try_from(len_i32).ok() else {
        diag(
            event_sink,
            &format!(
                "[brrmmmm] {action} key length {len_i32} exceeds configured limit of {max_key_bytes} bytes"
            ),
        );
        return Err(-1);
    };
    if len > max_key_bytes {
        diag(
            event_sink,
            &format!(
                "[brrmmmm] {action} key length {len} exceeds configured limit of {max_key_bytes} bytes"
            ),
        );
        return Err(-1);
    }
    let key_bytes = match read_memory_from_caller(caller, ptr, len_i32) {
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
    config: &Config,
    runtime_state: &Arc<Mutex<MissionRuntimeState>>,
    key: String,
    value: Vec<u8>,
    wasm_hash: Option<&str>,
    persist: PersistFn,
) -> std::result::Result<(), String> {
    if key.len() > config.limits.kv_max_key_bytes {
        return Err(format!(
            "kv_set key too large: {} bytes (max {} bytes per key)",
            key.len(),
            config.limits.kv_max_key_bytes
        ));
    }
    if value.len() > config.limits.kv_max_value_bytes {
        return Err(format!(
            "kv_set value too large: {} bytes (max {} bytes per key)",
            value.len(),
            config.limits.kv_max_value_bytes
        ));
    }

    let mut state = lock_runtime(runtime_state, "runtime_state");

    let existing_total: usize = state
        .kv
        .iter()
        .filter(|(k, _)| k.as_str() != key.as_str())
        .map(|(k, v)| k.len() + v.len())
        .sum();
    let new_total = existing_total
        .saturating_add(key.len())
        .saturating_add(value.len());
    if new_total > config.limits.kv_max_total_bytes {
        return Err(format!(
            "kv_set would exceed total KV budget: {new_total} bytes (max {} bytes total)",
            config.limits.kv_max_total_bytes
        ));
    }

    let previous = state.kv.insert(key.clone(), value);
    if let Err(error) = persist_if_host_persisted(config, &state, wasm_hash, persist) {
        restore_value(&mut state, key, previous);
        return Err(error);
    }
    drop(state);
    Ok(())
}

fn delete_value(
    config: &Config,
    runtime_state: &Arc<Mutex<MissionRuntimeState>>,
    key: &str,
    wasm_hash: Option<&str>,
    persist: PersistFn,
) -> std::result::Result<(), String> {
    let mut state = lock_runtime(runtime_state, "runtime_state");
    let previous = state.kv.remove(key);
    if previous.is_none() {
        drop(state);
        return Ok(());
    }
    if let Err(error) = persist_if_host_persisted(config, &state, wasm_hash, persist) {
        restore_value(&mut state, key.to_string(), previous);
        return Err(error);
    }
    drop(state);
    Ok(())
}

fn invalid_value_len(event_sink: &EventSink, value_len: i32, max_value_bytes: usize) -> i32 {
    diag(
        event_sink,
        &format!(
            "[brrmmmm] kv_set value length {value_len} exceeds configured limit of {max_value_bytes} bytes"
        ),
    );
    -1
}

fn restore_value(state: &mut MissionRuntimeState, key: String, previous: Option<Vec<u8>>) {
    if let Some(previous) = previous {
        state.kv.insert(key, previous);
    } else {
        state.kv.remove(&key);
    }
}

fn persist_if_host_persisted(
    config: &Config,
    state: &MissionRuntimeState,
    wasm_hash: Option<&str>,
    persist: PersistFn,
) -> std::result::Result<(), String> {
    let should_persist = state
        .describe
        .as_ref()
        .is_some_and(|describe| describe.state_persistence == PersistenceAuthority::HostPersisted);
    if !should_persist {
        return Ok(());
    }
    let wasm_hash =
        wasm_hash.ok_or_else(|| "host-persisted KV state has no WASM identity".to_string())?;
    persist(config, wasm_hash, state).map_err(|error| format!("{error:#}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::abi::MissionModuleDescribe;

    fn persisted_state() -> Arc<Mutex<MissionRuntimeState>> {
        let state = MissionRuntimeState {
            describe: Some(MissionModuleDescribe {
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
                operator_fallback: None,
            }),
            ..Default::default()
        };
        Arc::new(Mutex::new(state))
    }

    fn persist_ok(
        _config: &Config,
        _wasm_hash: &str,
        _state: &MissionRuntimeState,
    ) -> BrrmmmmResult<()> {
        std::fs::metadata(".")
            .map(|_| ())
            .map_err(|error| crate::error::BrrmmmmError::PersistenceFailure(error.to_string()))
    }

    fn persist_err(
        _config: &Config,
        _wasm_hash: &str,
        _state: &MissionRuntimeState,
    ) -> BrrmmmmResult<()> {
        Err(crate::error::BrrmmmmError::PersistenceFailure(
            "state directory is not writable".to_string(),
        ))
    }

    #[test]
    fn set_value_rolls_back_new_key_when_persistence_fails() {
        let state = persisted_state();
        let config = Config::load().expect("test config");

        let result = set_value(
            &config,
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
        let config = Config::load().expect("test config");
        lock_runtime(&state, "runtime_state")
            .kv
            .insert("token".to_string(), b"old".to_vec());

        let result = set_value(
            &config,
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
        let config = Config::load().expect("test config");
        lock_runtime(&state, "runtime_state")
            .kv
            .insert("token".to_string(), b"secret".to_vec());

        let result = delete_value(&config, &state, "token", Some("wasm-hash"), persist_err);

        assert!(result.is_err());
        assert_eq!(
            lock_runtime(&state, "runtime_state").kv.get("token"),
            Some(&b"secret".to_vec())
        );
    }

    #[test]
    fn delete_value_missing_key_does_not_require_persistence() {
        let state = persisted_state();
        let config = Config::load().expect("test config");

        let result = delete_value(&config, &state, "missing", Some("wasm-hash"), persist_err);

        assert!(result.is_ok());
    }

    #[test]
    fn set_value_rejects_oversized_single_value() {
        let state = persisted_state();
        let config = Config::load().expect("test config");
        let oversized = vec![0u8; config.limits.kv_max_value_bytes + 1];

        let result = set_value(
            &config,
            &state,
            "big".to_string(),
            oversized,
            Some("wasm-hash"),
            persist_ok,
        );

        assert!(result.unwrap_err().contains("too large"));
    }

    #[test]
    fn set_value_rejects_oversized_key() {
        let state = persisted_state();
        let config = Config::load().expect("test config");
        let key = "k".repeat(config.limits.kv_max_key_bytes + 1);

        let result = set_value(
            &config,
            &state,
            key,
            b"value".to_vec(),
            Some("wasm-hash"),
            persist_ok,
        );

        assert!(result.unwrap_err().contains("key too large"));
    }

    #[test]
    fn set_value_rejects_when_total_budget_exceeded() {
        let state = persisted_state();
        let config = Config::load().expect("test config");
        // Fill the map near capacity. Key bytes count toward the total budget.
        for i in 0..15usize {
            set_value(
                &config,
                &state,
                format!("key{i}"),
                vec![0u8; config.limits.kv_max_value_bytes],
                Some("wasm-hash"),
                persist_ok,
            )
            .unwrap_or_else(|e| panic!("key{i} should fit: {e}"));
        }

        // Another max-sized value must be rejected.
        let result = set_value(
            &config,
            &state,
            "overflow".to_string(),
            vec![0u8; config.limits.kv_max_value_bytes],
            Some("wasm-hash"),
            persist_ok,
        );

        assert!(result.unwrap_err().contains("total KV budget"));
    }

    #[test]
    fn set_value_replacing_existing_key_does_not_double_count_old_bytes() {
        let state = persisted_state();
        let config = Config::load().expect("test config");
        // Fill the map near capacity. Key bytes count toward the total budget.
        for i in 0..15usize {
            set_value(
                &config,
                &state,
                format!("key{i}"),
                vec![0u8; config.limits.kv_max_value_bytes],
                Some("wasm-hash"),
                persist_ok,
            )
            .unwrap_or_else(|e| panic!("key{i} should fit: {e}"));
        }

        // Replace an existing key with a smaller value — this should not exceed the budget.
        let result = set_value(
            &config,
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
        let config = Config::load().expect("test config");

        let result = set_value(
            &config,
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
