//! State persistence for sidecar runtime state.
//!
//! State is keyed by a stable hash of the WASM binary, so changing the binary
//! starts fresh. Stored in `~/.local/share/brrmmmm/state/{hash}.json`.
//!
//! ## Persistence classes
//!
//! - `volatile`: nothing to persist (RAM only).
//! - `host_persisted`: this module saves JSON to disk. Restart-safe, not abuse-safe.
//! - `vendor_backed`: not implemented; requires server-issued tokens.

use std::path::PathBuf;

use anyhow::Context;

use crate::abi::SidecarRuntimeState;

// ── Hashing ──────────────────────────────────────────────────────────

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

// ── Storage path ──────────────────────────────────────────────────────

fn state_dir() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("BRRMMMM_STATE_DIR") {
        return Some(PathBuf::from(path));
    }
    let home = std::env::var_os("HOME")?;
    let mut path = PathBuf::from(home);
    path.push(".local");
    path.push("share");
    path.push("brrmmmm");
    path.push("state");
    Some(path)
}

fn state_path(hash: &str) -> Option<PathBuf> {
    let mut path = state_dir()?;
    path.push(format!("{hash}.json"));
    Some(path)
}

// ── Public API ────────────────────────────────────────────────────────

/// Load persisted runtime state for a WASM module identified by `wasm_hash`.
/// Returns `None` if no state file exists or if deserialization fails.
#[allow(dead_code)]
pub fn load(wasm_hash: &str) -> Option<SidecarRuntimeState> {
    let path = state_path(wasm_hash)?;
    let data = std::fs::read(&path).ok()?;
    serde_json::from_slice(&data).ok()
}

/// Persist runtime state for a WASM module identified by `wasm_hash`.
pub fn save(wasm_hash: &str, state: &SidecarRuntimeState) -> anyhow::Result<()> {
    let path = state_path(wasm_hash).context("resolve brrmmmm state path")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("create brrmmmm state directory: {}", dir.display()))?;
    }
    let json = serde_json::to_vec_pretty(state).context("serialize brrmmmm runtime state")?;
    std::fs::write(&path, json)
        .with_context(|| format!("write brrmmmm state file: {}", path.display()))?;
    Ok(())
}

/// Remove persisted state for a WASM module.
#[allow(dead_code)]
pub fn clear(wasm_hash: &str) {
    if let Some(path) = state_path(wasm_hash) {
        let _ = std::fs::remove_file(path);
    }
}
