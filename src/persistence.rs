//! State persistence for sidecar runtime state.
//!
//! Runtime state is keyed by a stable hash of the WASM binary, so changing the
//! binary starts fresh. Stored in the configured state directory as `{hash}.json`.
//!
//! Loading distinguishes absent state from corrupted state:
//!
//! - `Ok(None)`: no state file exists.
//! - `Err`: the state file exists but is unreadable, invalid, or over quota.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use crate::abi::SidecarRuntimeState;
use crate::config::{Config, RuntimeLimits};
use crate::error::{BrrmmmmError, BrrmmmmResult};

/// Compute a stable FNV-1a 64-bit hash of `data` and return it as a hex string.
///
/// Non-cryptographic but deterministic and dependency-free.
pub fn wasm_identity(data: &[u8]) -> String {
    let mut hash: u64 = 14_695_981_039_346_656_037;
    for &byte in data {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(1_099_511_628_211);
    }
    format!("{hash:016x}")
}

fn state_path(config: &Config, hash: &str) -> PathBuf {
    config.state_dir.join(format!("{hash}.json"))
}

/// Load persisted runtime state for a WASM module identified by `wasm_hash`.
pub fn load(config: &Config, wasm_hash: &str) -> BrrmmmmResult<Option<SidecarRuntimeState>> {
    let path = state_path(config, wasm_hash);
    let data = match std::fs::read(&path) {
        Ok(data) => data,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(BrrmmmmError::PersistenceFailure(format!(
                "read brrmmmm state file {}: {error}",
                path.display()
            )));
        }
    };
    let state: SidecarRuntimeState = serde_json::from_slice(&data).map_err(|error| {
        BrrmmmmError::StateCorruption(format!("decode {}: {error}", path.display()))
    })?;
    validate_state(&state, &config.limits)?;
    Ok(Some(state))
}

/// Persist runtime state for a WASM module identified by `wasm_hash`.
pub fn save(config: &Config, wasm_hash: &str, state: &SidecarRuntimeState) -> BrrmmmmResult<()> {
    validate_state(state, &config.limits)?;
    let path = state_path(config, wasm_hash);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|error| {
            BrrmmmmError::PersistenceFailure(format!(
                "create brrmmmm state directory {}: {error}",
                dir.display()
            ))
        })?;
    }
    let json = serde_json::to_vec_pretty(state).map_err(|error| {
        BrrmmmmError::PersistenceFailure(format!("serialize brrmmmm runtime state: {error}"))
    })?;
    atomic_write(&path, &json, FileMode::Private)
}

/// Remove persisted state for a WASM module.
#[allow(dead_code)]
pub fn clear(config: &Config, wasm_hash: &str) {
    let path = state_path(config, wasm_hash);
    let _ = std::fs::remove_file(path);
}

pub(crate) fn validate_state(
    state: &SidecarRuntimeState,
    limits: &RuntimeLimits,
) -> BrrmmmmResult<()> {
    let mut total = 0usize;
    for (key, value) in &state.kv {
        let key_len = key.len();
        if key_len > limits.kv_max_key_bytes {
            return Err(BrrmmmmError::budget(
                "kv key",
                key_len,
                limits.kv_max_key_bytes,
            ));
        }
        if value.len() > limits.kv_max_value_bytes {
            return Err(BrrmmmmError::budget(
                "kv value",
                value.len(),
                limits.kv_max_value_bytes,
            ));
        }
        total = total.saturating_add(key_len).saturating_add(value.len());
        if total > limits.kv_max_total_bytes {
            return Err(BrrmmmmError::budget(
                "kv total",
                total,
                limits.kv_max_total_bytes,
            ));
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
pub(crate) enum FileMode {
    Private,
}

pub(crate) fn atomic_write(path: &Path, data: &[u8], mode: FileMode) -> BrrmmmmResult<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent).map_err(|error| {
        BrrmmmmError::PersistenceFailure(format!("create directory {}: {error}", parent.display()))
    })?;

    let mut tmp_path = None;
    let mut tmp_file = None;
    for attempt in 0..32u32 {
        let candidate = parent.join(format!(
            ".{}.{}.{}.tmp",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("state"),
            std::process::id(),
            attempt
        ));
        match open_temp(&candidate, mode) {
            Ok(file) => {
                tmp_path = Some(candidate);
                tmp_file = Some(file);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(BrrmmmmError::PersistenceFailure(format!(
                    "open temp file {}: {error}",
                    candidate.display()
                )));
            }
        }
    }

    let tmp_path = tmp_path.ok_or_else(|| {
        BrrmmmmError::PersistenceFailure(format!(
            "allocate temp file next to {} after 32 attempts",
            path.display()
        ))
    })?;
    let mut file = tmp_file.expect("tmp_file set with tmp_path");

    let result = (|| -> BrrmmmmResult<()> {
        file.write_all(data).map_err(|error| {
            BrrmmmmError::PersistenceFailure(format!("write {}: {error}", tmp_path.display()))
        })?;
        file.sync_all().map_err(|error| {
            BrrmmmmError::PersistenceFailure(format!("fsync {}: {error}", tmp_path.display()))
        })?;
        drop(file);
        std::fs::rename(&tmp_path, path).map_err(|error| {
            BrrmmmmError::PersistenceFailure(format!(
                "rename {} to {}: {error}",
                tmp_path.display(),
                path.display()
            ))
        })?;
        fsync_dir(parent)?;
        Ok(())
    })();

    if result.is_err() {
        let _ = std::fs::remove_file(&tmp_path);
    }
    result
}

#[cfg(unix)]
fn open_temp(path: &Path, mode: FileMode) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt as _;

    let mode = match mode {
        FileMode::Private => 0o600,
    };
    std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(mode)
        .open(path)
}

#[cfg(not(unix))]
fn open_temp(path: &Path, _mode: FileMode) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
}

pub(crate) fn fsync_dir(path: &Path) -> BrrmmmmResult<()> {
    match std::fs::File::open(path) {
        Ok(file) => file.sync_all().map_err(|error| {
            BrrmmmmError::PersistenceFailure(format!("fsync directory {}: {error}", path.display()))
        }),
        Err(error) => Err(BrrmmmmError::PersistenceFailure(format!(
            "open directory {} for fsync: {error}",
            path.display()
        ))),
    }
}
