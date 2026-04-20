use std::sync::{Arc, Mutex};

use anyhow::Result;

use crate::host::HostState;

use super::super::io::{
    WasmCaller, WasmLinker, lock_runtime, read_memory_from_caller, write_memory_from_caller,
};

pub(super) fn register(linker: &mut WasmLinker, shared: Arc<Mutex<HostState>>) -> Result<()> {
    let shared_len = shared.clone();
    let shared_get = shared.clone();
    let shared_set = shared.clone();
    let shared_visibility = shared;

    linker.func_wrap(
        "vzglyd_host",
        "ua_get_len",
        move |_caller: WasmCaller<'_>| -> i32 { user_agent_len(&shared_len) },
    )?;

    linker.func_wrap(
        "vzglyd_host",
        "ua_get",
        move |mut caller: WasmCaller<'_>, ptr: i32, len: i32| -> i32 {
            let ua_bytes = current_user_agent_bytes(&shared_get);
            let to_write = &ua_bytes[..ua_bytes.len().min(len as usize)];
            match write_memory_from_caller(&mut caller, ptr, to_write) {
                Ok(()) => to_write.len() as i32,
                Err(_) => -1,
            }
        },
    )?;

    linker.func_wrap(
        "vzglyd_host",
        "ua_set",
        move |mut caller: WasmCaller<'_>, ptr: i32, len: i32| -> i32 {
            let bytes = match read_memory_from_caller(&mut caller, ptr, len) {
                Ok(b) => b,
                Err(_) => return -1,
            };
            set_user_agent_bytes(&shared_set, bytes)
        },
    )?;

    linker.func_wrap(
        "vzglyd_host",
        "identity_disclosure_set",
        move |visible: i32| -> i32 { set_identity_disclosure(&shared_visibility, visible != 0) },
    )?;

    Ok(())
}

fn user_agent_len(shared: &Arc<Mutex<HostState>>) -> i32 {
    current_user_agent_bytes(shared).len() as i32
}

fn current_user_agent_bytes(shared: &Arc<Mutex<HostState>>) -> Vec<u8> {
    let s = lock_runtime(shared, "host_state");
    s.user_agent
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_bytes()
        .to_vec()
}

fn set_user_agent_bytes(shared: &Arc<Mutex<HostState>>, bytes: Vec<u8>) -> i32 {
    match String::from_utf8(bytes) {
        Ok(new_ua) => {
            let s = lock_runtime(shared, "host_state");
            *s.user_agent.lock().unwrap_or_else(|e| e.into_inner()) = new_ua;
            0
        }
        Err(_) => -1,
    }
}

fn set_identity_disclosure(shared: &Arc<Mutex<HostState>>, visible: bool) -> i32 {
    let mut s = lock_runtime(shared, "host_state");
    s.set_identity_disclosure_visible(visible);
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host() -> Arc<Mutex<HostState>> {
        Arc::new(Mutex::new(HostState::new(
            false,
            Arc::new(Mutex::new(None)),
            [0u8; 32],
            None,
        )))
    }

    #[test]
    fn ua_set_helper_replaces_sidecar_user_agent() {
        let shared = host();

        assert_eq!(set_user_agent_bytes(&shared, b"sidecar/ci".to_vec()), 0);

        assert_eq!(user_agent_len(&shared), "sidecar/ci".len() as i32);
        assert_eq!(current_user_agent_bytes(&shared), b"sidecar/ci");
    }

    #[test]
    fn ua_set_helper_rejects_invalid_utf8() {
        let shared = host();

        assert_eq!(set_user_agent_bytes(&shared, vec![0xff]), -1);
    }

    #[test]
    fn identity_disclosure_helper_toggles_runtime_policy() {
        let shared = host();

        assert_eq!(set_identity_disclosure(&shared, false), 0);
        assert!(!shared.lock().unwrap().identity_disclosure_visible);
        assert_eq!(set_identity_disclosure(&shared, true), 0);
        assert!(shared.lock().unwrap().identity_disclosure_visible);
    }
}
