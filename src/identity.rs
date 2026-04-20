use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};

use crate::attestation::{base64url, decode_base64url, short_hash_16};
use crate::events::now_ms;

#[derive(Clone)]
pub struct InstallationIdentity {
    pub client_id: [u8; 16],
    pub key_id: [u8; 16],
    pub public_key: [u8; 32],
    pub(crate) signing_key: SigningKey,
}

#[derive(Debug, Serialize, Deserialize)]
struct IdentityJson {
    version: u8,
    installation_id: String,
    client_id: String,
    created_at_ms: u64,
}

pub fn attestation_disabled() -> bool {
    std::env::var("BRRMMMM_ATTESTATION")
        .map(|value| {
            let value = value.trim();
            value == "0"
                || value.eq_ignore_ascii_case("off")
                || value.eq_ignore_ascii_case("false")
                || value.eq_ignore_ascii_case("no")
                || value.eq_ignore_ascii_case("legacy")
        })
        .unwrap_or(false)
}

pub fn load_or_create() -> Result<InstallationIdentity> {
    let dir = identity_dir().context("resolve brrmmmm identity path")?;
    load_or_create_at(&dir)
}

pub fn load_or_create_at(dir: &Path) -> Result<InstallationIdentity> {
    let identity_path = dir.join("identity.json");
    let private_key_path = dir.join("private_key.bin");
    let public_key_path = dir.join("public_key.bin");

    if identity_path.exists() || private_key_path.exists() || public_key_path.exists() {
        return load_existing(&identity_path, &private_key_path, &public_key_path);
    }

    std::fs::create_dir_all(dir)
        .with_context(|| format!("create brrmmmm identity directory: {}", dir.display()))?;
    let mut installation_id = [0u8; 32];
    getrandom::fill(&mut installation_id)
        .map_err(|error| anyhow::anyhow!("generate brrmmmm installation id: {error}"))?;
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed)
        .map_err(|error| anyhow::anyhow!("generate brrmmmm signing key: {error}"))?;
    let signing_key = SigningKey::from_bytes(&seed);
    let public_key = signing_key.verifying_key().to_bytes();
    let client_id = derive_client_id(&installation_id);
    let identity = IdentityJson {
        version: 1,
        installation_id: base64url(&installation_id),
        client_id: base64url(&client_id),
        created_at_ms: now_ms(),
    };
    let json = serde_json::to_vec_pretty(&identity).context("serialize brrmmmm identity")?;
    write_private(&identity_path, &json)
        .with_context(|| format!("write {}", identity_path.display()))?;
    write_private(&private_key_path, &seed)
        .with_context(|| format!("write {}", private_key_path.display()))?;
    write_private(&public_key_path, &public_key)
        .with_context(|| format!("write {}", public_key_path.display()))?;

    Ok(InstallationIdentity {
        client_id,
        key_id: short_hash_16(b"brrmmmm-key-id-v1", &public_key),
        public_key,
        signing_key,
    })
}

fn load_existing(
    identity_path: &Path,
    private_key_path: &Path,
    public_key_path: &Path,
) -> Result<InstallationIdentity> {
    let identity_bytes = std::fs::read(identity_path)
        .with_context(|| format!("read {}", identity_path.display()))?;
    let identity: IdentityJson =
        serde_json::from_slice(&identity_bytes).context("decode brrmmmm identity.json")?;
    anyhow::ensure!(
        identity.version == 1,
        "unsupported brrmmmm identity version {}",
        identity.version
    );
    let installation_id =
        parse_array::<32>(&identity.installation_id).context("decode brrmmmm installation id")?;
    let client_id = derive_client_id(&installation_id);
    let stored_client_id =
        parse_array::<16>(&identity.client_id).context("decode brrmmmm client id")?;
    anyhow::ensure!(
        stored_client_id == client_id,
        "brrmmmm identity client_id does not match installation_id"
    );

    let seed: [u8; 32] = std::fs::read(private_key_path)
        .with_context(|| format!("read {}", private_key_path.display()))?
        .try_into()
        .map_err(|bytes: Vec<u8>| {
            anyhow::anyhow!("expected 32-byte private key seed, got {}", bytes.len())
        })?;
    let signing_key = SigningKey::from_bytes(&seed);
    let public_key = signing_key.verifying_key().to_bytes();
    let stored_public_key: [u8; 32] = std::fs::read(public_key_path)
        .with_context(|| format!("read {}", public_key_path.display()))?
        .try_into()
        .map_err(|bytes: Vec<u8>| {
            anyhow::anyhow!("expected 32-byte public key, got {}", bytes.len())
        })?;
    if stored_public_key != public_key {
        write_private(public_key_path, &public_key)
            .with_context(|| format!("repair {}", public_key_path.display()))?;
    }

    Ok(InstallationIdentity {
        client_id,
        key_id: short_hash_16(b"brrmmmm-key-id-v1", &public_key),
        public_key,
        signing_key,
    })
}

fn derive_client_id(installation_id: &[u8; 32]) -> [u8; 16] {
    short_hash_16(b"brrmmmm-client-id-v1", installation_id)
}

fn identity_dir() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("BRRMMMM_IDENTITY_DIR") {
        return Some(PathBuf::from(path));
    }
    let home = std::env::var_os("HOME")?;
    let mut path = PathBuf::from(home);
    path.push(".local");
    path.push("share");
    path.push("brrmmmm");
    path.push("identity");
    Some(path)
}

fn parse_array<const N: usize>(value: &str) -> Result<[u8; N]> {
    decode_base64url(value)
        .map_err(|error| anyhow::anyhow!("{error}"))?
        .try_into()
        .map_err(|bytes: Vec<u8>| anyhow::anyhow!("expected {N} bytes, got {}", bytes.len()))
}

#[cfg(unix)]
fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(data)
}

#[cfg(not(unix))]
fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, data)
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn temp_identity_dir() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "brrmmmm-identity-test-{}-{nanos}",
            std::process::id()
        ))
    }

    #[test]
    fn identity_is_created_and_reused() {
        let dir = temp_identity_dir();
        let first = load_or_create_at(&dir).unwrap();
        let second = load_or_create_at(&dir).unwrap();

        assert_eq!(first.client_id, second.client_id);
        assert_eq!(first.key_id, second.key_id);
        assert_eq!(first.public_key, second.public_key);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn deleting_identity_regenerates_client_id() {
        let dir = temp_identity_dir();
        let first = load_or_create_at(&dir).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        let second = load_or_create_at(&dir).unwrap();

        assert_ne!(first.client_id, second.client_id);

        let _ = std::fs::remove_dir_all(dir);
    }
}
