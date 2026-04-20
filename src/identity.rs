use std::fmt;
use std::path::Path;
use std::str::FromStr;

use anyhow::{Context, Result};
use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};

use crate::events::now_ms;
use crate::utils::{base64url, decode_base64url, short_hash_16};

macro_rules! define_id_type {
    ($name:ident, $size:expr, $label:expr) => {
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub [u8; $size]);

        impl $name {
            pub fn from_bytes(bytes: [u8; $size]) -> Self {
                Self(bytes)
            }

            pub fn as_bytes(&self) -> &[u8; $size] {
                &self.0
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), base64url(&self.0))
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", base64url(&self.0))
            }
        }

        impl FromStr for $name {
            type Err = anyhow::Error;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                let bytes = decode_base64url(s)
                    .map_err(|e| anyhow::anyhow!("invalid {}: {}", $label, e))?;
                let array: [u8; $size] = bytes.try_into().map_err(|bytes: Vec<u8>| {
                    anyhow::anyhow!("expected {} bytes for {}, got {}", $size, $label, bytes.len())
                })?;
                Ok(Self(array))
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                serializer.serialize_str(&base64url(&self.0))
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let s = String::deserialize(deserializer)?;
                Self::from_str(&s).map_err(serde::de::Error::custom)
            }
        }
    };
}

define_id_type!(ClientId, 16, "client_id");
define_id_type!(KeyId, 16, "key_id");
define_id_type!(InstallationId, 32, "installation_id");
define_id_type!(PublicKey, 32, "public_key");
define_id_type!(MissionId, 16, "mission_id");
define_id_type!(ModuleHash, 32, "module_hash");
define_id_type!(BehaviorHash, 16, "behavior_hash");

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IdentityMetadata {
    pub client_id: ClientId,
    pub key_id: KeyId,
    pub public_key: PublicKey,
}

#[derive(Clone)]
pub struct InstallationIdentity {
    pub metadata: IdentityMetadata,
    signing_key: SigningKey,
}

impl InstallationIdentity {
    #[cfg(test)]
    pub(crate) fn new_for_test(seed: [u8; 32]) -> Self {
        let signing_key = SigningKey::from_bytes(&seed);
        let public_key = PublicKey(signing_key.verifying_key().to_bytes());
        let installation_id = InstallationId([0u8; 32]);
        let client_id = derive_client_id(&installation_id);
        Self {
            metadata: IdentityMetadata {
                client_id,
                key_id: KeyId(short_hash_16(b"brrmmmm-key-id-v1", public_key.as_bytes())),
                public_key,
            },
            signing_key,
        }
    }

    pub fn client_id(&self) -> ClientId {
        self.metadata.client_id
    }

    pub fn key_id(&self) -> KeyId {
        self.metadata.key_id
    }

    pub fn public_key(&self) -> PublicKey {
        self.metadata.public_key
    }

    pub fn sign_message(&self, message: &[u8]) -> ed25519_dalek::Signature {
        use ed25519_dalek::Signer;
        self.signing_key.sign(message)
    }
}

impl ed25519_dalek::Signer<ed25519_dalek::Signature> for InstallationIdentity {
    fn try_sign(&self, message: &[u8]) -> Result<ed25519_dalek::Signature, ed25519_dalek::SignatureError> {
        self.signing_key.try_sign(message)
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct IdentityJson {
    version: u8,
    installation_id: InstallationId,
    client_id: ClientId,
    created_at_ms: u64,
}

#[derive(thiserror::Error, Debug)]
pub enum IdentityError {
    #[error("identity not found")]
    NotFound,
    #[error("corrupted identity: {0}")]
    Corrupted(String),
    #[error("unsupported version: {0}")]
    UnsupportedVersion(u8),
    #[error("mismatched public key")]
    PublicKeyMismatch,
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub fn load_or_create(config: &crate::config::Config) -> Result<InstallationIdentity> {
    load_or_create_at(&config.identity_dir)
}

pub fn load_or_create_at(dir: &Path) -> Result<InstallationIdentity> {
    match load_existing_at(dir) {
        Ok(identity) => Ok(identity),
        Err(IdentityError::NotFound) => create_new_at(dir),
        Err(IdentityError::PublicKeyMismatch) => {
            // Repair public key if needed
            let identity = load_existing_at_ignoring_mismatch(dir)
                .map_err(|e| anyhow::anyhow!("repair failed: {e}"))?;
            let public_key_path = dir.join("public_key.bin");
            write_private(&public_key_path, identity.public_key().as_bytes())
                .with_context(|| format!("repair {}", public_key_path.display()))?;
            Ok(identity)
        }
        Err(e) => Err(e.into()),
    }
}

fn load_existing_at(dir: &Path) -> Result<InstallationIdentity, IdentityError> {
    let identity = load_existing_at_ignoring_mismatch(dir)?;

    let public_key_path = dir.join("public_key.bin");
    let stored_public_key_bytes: [u8; 32] = std::fs::read(&public_key_path)
        .with_context(|| format!("read {}", public_key_path.display()))?
        .try_into()
        .map_err(|bytes: Vec<u8>| {
            anyhow::anyhow!("expected 32-byte public key, got {}", bytes.len())
        })?;
    let stored_public_key = PublicKey(stored_public_key_bytes);

    if stored_public_key != identity.public_key() {
        return Err(IdentityError::PublicKeyMismatch);
    }

    Ok(identity)
}

fn load_existing_at_ignoring_mismatch(dir: &Path) -> Result<InstallationIdentity, IdentityError> {
    let identity_path = dir.join("identity.json");
    let private_key_path = dir.join("private_key.bin");

    if !identity_path.exists() || !private_key_path.exists() {
        return Err(IdentityError::NotFound);
    }

    let identity_bytes = std::fs::read(&identity_path)
        .with_context(|| format!("read {}", identity_path.display()))?;
    let identity: IdentityJson = serde_json::from_slice(&identity_bytes)
        .map_err(|e| IdentityError::Corrupted(format!("decode identity.json: {e}")))?;

    if identity.version != 1 {
        return Err(IdentityError::UnsupportedVersion(identity.version));
    }

    let client_id = derive_client_id(&identity.installation_id);
    if identity.client_id != client_id {
        return Err(IdentityError::Corrupted(
            "client_id does not match installation_id".to_string(),
        ));
    }

    let seed: [u8; 32] = std::fs::read(&private_key_path)
        .with_context(|| format!("read {}", private_key_path.display()))?
        .try_into()
        .map_err(|bytes: Vec<u8>| {
            anyhow::anyhow!("expected 32-byte private key seed, got {}", bytes.len())
        })?;
    let signing_key = SigningKey::from_bytes(&seed);
    let public_key = PublicKey(signing_key.verifying_key().to_bytes());

    Ok(InstallationIdentity {
        metadata: IdentityMetadata {
            client_id,
            key_id: KeyId(short_hash_16(b"brrmmmm-key-id-v1", public_key.as_bytes())),
            public_key,
        },
        signing_key,
    })
}

fn create_new_at(dir: &Path) -> Result<InstallationIdentity> {
    log::trace!("generating new cryptographic identity at {}", dir.display());

    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create parent directory: {}", parent.display()))?;
    }

    // Create a temporary directory in the same parent
    let tmp_dir_name = format!(
        ".tmp-identity-{}",
        base64url(&short_hash_16(b"tmp", now_ms().to_be_bytes().as_slice()))
    );
    let tmp_dir = dir.parent().unwrap_or_else(|| Path::new(".")).join(tmp_dir_name);
    std::fs::create_dir_all(&tmp_dir)
        .with_context(|| format!("create temporary identity directory: {}", tmp_dir.display()))?;

    let result = (|| -> Result<InstallationIdentity> {
        let mut raw_installation_id = [0u8; 32];
        getrandom::fill(&mut raw_installation_id)
            .map_err(|error| anyhow::anyhow!("generate installation id: {error}"))?;
        log::trace!("entropy source initialized for installation_id");
        let installation_id = InstallationId(raw_installation_id);

        let mut seed = [0u8; 32];
        getrandom::fill(&mut seed)
            .map_err(|error| anyhow::anyhow!("generate signing key: {error}"))?;
        log::trace!("entropy source initialized for signing_key");
        let signing_key = SigningKey::from_bytes(&seed);
        let public_key = PublicKey(signing_key.verifying_key().to_bytes());
        let client_id = derive_client_id(&installation_id);

        let identity = IdentityJson {
            version: 1,
            installation_id,
            client_id,
            created_at_ms: now_ms(),
        };

        let json = serde_json::to_vec_pretty(&identity).context("serialize identity")?;
        write_private(&tmp_dir.join("identity.json"), &json)?;
        write_private(&tmp_dir.join("private_key.bin"), &seed)?;
        write_private(&tmp_dir.join("public_key.bin"), public_key.as_bytes())?;

        Ok(InstallationIdentity {
            metadata: IdentityMetadata {
                client_id,
                key_id: KeyId(short_hash_16(b"brrmmmm-key-id-v1", public_key.as_bytes())),
                public_key,
            },
            signing_key,
        })
    })();

    match result {
        Ok(identity) => {
            // Atomic move
            if dir.exists() {
                let _ = std::fs::remove_dir_all(dir);
            }
            std::fs::rename(&tmp_dir, dir)
                .with_context(|| format!("move {} to {}", tmp_dir.display(), dir.display()))?;
            Ok(identity)
        }
        Err(e) => {
            let _ = std::fs::remove_dir_all(&tmp_dir);
            Err(e)
        }
    }
}

fn derive_client_id(installation_id: &InstallationId) -> ClientId {
    ClientId(short_hash_16(
        b"brrmmmm-client-id-v1",
        installation_id.as_bytes(),
    ))
}

#[cfg(unix)]
fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("open {}", path.display()))?;
    file.write_all(data)
        .with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

#[cfg(not(unix))]
fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    std::fs::write(path, data).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newtypes_roundtrip_base64() {
        let original = ClientId([1u8; 16]);
        let s = original.to_string();
        let decoded = ClientId::from_str(&s).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn load_or_create_at_works() {
        let dir = std::env::temp_dir().join(format!("brrmmmm-test-identity-{}", now_ms()));
        let first = load_or_create_at(&dir).unwrap();
        let second = load_or_create_at(&dir).unwrap();

        assert_eq!(first.client_id(), second.client_id());
        assert_eq!(first.public_key(), second.public_key());

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn repair_recreates_public_key() {
        let dir = std::env::temp_dir().join(format!("brrmmmm-test-repair-{}", now_ms()));
        let identity = load_or_create_at(&dir).unwrap();
        let public_key_path = dir.join("public_key.bin");

        // Corrupt public key
        std::fs::write(&public_key_path, [0u8; 32]).unwrap();

        let repaired = load_or_create_at(&dir).unwrap();
        assert_eq!(repaired.public_key(), identity.public_key());

        let _ = std::fs::remove_dir_all(dir);
    }
}
