use std::fmt;
use std::path::Path;
use std::str::FromStr;

use anyhow::{Context, Result};
use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::events::now_ms;
use crate::persistence::{FileMode, atomic_write, fsync_dir};
use crate::utils::{base64url, decode_base64url, short_hash_16};

macro_rules! define_id_type {
    ($name:ident, $size:expr, $label:expr) => {
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub [u8; $size]);

        impl $name {
            pub const fn as_bytes(&self) -> &[u8; $size] {
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
                    anyhow::anyhow!(
                        "expected {} bytes for {}, got {}",
                        $size,
                        $label,
                        bytes.len()
                    )
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
define_id_type!(KeyId, 32, "key_id");
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
pub struct IdentitySigner {
    signing_key: SigningKey,
}

impl IdentitySigner {
    const fn new(signing_key: SigningKey) -> Self {
        Self { signing_key }
    }
}

#[derive(Clone)]
pub struct InstallationIdentity {
    pub metadata: IdentityMetadata,
    signer: IdentitySigner,
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
                key_id: derive_key_id(&public_key),
                public_key,
            },
            signer: IdentitySigner::new(signing_key),
        }
    }

    pub const fn client_id(&self) -> ClientId {
        self.metadata.client_id
    }

    pub const fn key_id(&self) -> KeyId {
        self.metadata.key_id
    }

    pub const fn public_key(&self) -> PublicKey {
        self.metadata.public_key
    }
}

impl ed25519_dalek::Signer<ed25519_dalek::Signature> for InstallationIdentity {
    fn try_sign(
        &self,
        message: &[u8],
    ) -> Result<ed25519_dalek::Signature, ed25519_dalek::SignatureError> {
        self.signer.signing_key.try_sign(message)
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
        Err(e) => Err(e.into()),
    }
}

fn load_existing_at(dir: &Path) -> Result<InstallationIdentity, IdentityError> {
    let identity = load_existing_at_ignoring_mismatch(dir)?;
    ensure_public_key_file(dir, identity.public_key())?;
    Ok(identity)
}

fn ensure_public_key_file(dir: &Path, expected_public_key: PublicKey) -> Result<()> {
    let public_key_path = dir.join("public_key.bin");
    match read_stored_public_key(&public_key_path) {
        Ok(stored_public_key) if stored_public_key == expected_public_key => Ok(()),
        Ok(_) => repair_public_key_file(&public_key_path, expected_public_key, "stale"),
        Err(error) => repair_public_key_file(
            &public_key_path,
            expected_public_key,
            &format!("invalid ({error:#})"),
        ),
    }
}

fn read_stored_public_key(path: &Path) -> Result<PublicKey> {
    let stored_public_key_bytes: [u8; 32] = std::fs::read(path)
        .with_context(|| format!("read {}", path.display()))?
        .try_into()
        .map_err(|bytes: Vec<u8>| {
            anyhow::anyhow!("expected 32-byte public key, got {}", bytes.len())
        })?;
    Ok(PublicKey(stored_public_key_bytes))
}

fn repair_public_key_file(path: &Path, expected_public_key: PublicKey, reason: &str) -> Result<()> {
    tracing::warn!(
        path = %path.display(),
        reason,
        "repairing derived identity public key file"
    );
    write_private(path, expected_public_key.as_bytes())
        .with_context(|| format!("repair {}", path.display()))
}

fn load_existing_at_ignoring_mismatch(dir: &Path) -> Result<InstallationIdentity, IdentityError> {
    let identity_path = dir.join("identity.json");
    let private_key_path = dir.join("private_key.bin");
    let public_key_path = dir.join("public_key.bin");

    if !dir.exists() {
        return Err(IdentityError::NotFound);
    }
    let identity_exists = identity_path.exists();
    let private_key_exists = private_key_path.exists();
    let public_key_exists = public_key_path.exists();
    if !identity_exists && !private_key_exists && !public_key_exists {
        return Err(IdentityError::NotFound);
    }
    if !identity_exists || !private_key_exists || !public_key_exists {
        return Err(IdentityError::Corrupted(
            "identity directory is incomplete".to_string(),
        ));
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
            key_id: derive_key_id(&public_key),
            public_key,
        },
        signer: IdentitySigner::new(signing_key),
    })
}

fn create_new_at(dir: &Path) -> Result<InstallationIdentity> {
    tracing::trace!("generating new cryptographic identity at {}", dir.display());

    let parent = dir.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)
        .with_context(|| format!("create parent directory: {}", parent.display()))?;

    let tmp_dir = create_temp_identity_dir(parent)?;

    let result = (|| -> Result<InstallationIdentity> {
        let mut raw_installation_id = [0u8; 32];
        getrandom::fill(&mut raw_installation_id)
            .map_err(|error| anyhow::anyhow!("generate installation id: {error}"))?;
        tracing::trace!("entropy source initialized for installation_id");
        let installation_id = InstallationId(raw_installation_id);

        let mut seed = [0u8; 32];
        getrandom::fill(&mut seed)
            .map_err(|error| anyhow::anyhow!("generate signing key: {error}"))?;
        tracing::trace!("entropy source initialized for signing_key");
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
                key_id: derive_key_id(&public_key),
                public_key,
            },
            signer: IdentitySigner::new(signing_key),
        })
    })();

    match result {
        Ok(identity) => {
            if dir.exists() {
                let _ = std::fs::remove_dir_all(&tmp_dir);
                anyhow::bail!(
                    "identity directory already exists while creating: {}",
                    dir.display()
                );
            }
            std::fs::rename(&tmp_dir, dir)
                .with_context(|| format!("move {} to {}", tmp_dir.display(), dir.display()))?;
            fsync_dir(parent).map_err(anyhow::Error::new)?;
            Ok(identity)
        }
        Err(e) => {
            let _ = std::fs::remove_dir_all(&tmp_dir);
            Err(e)
        }
    }
}

fn create_temp_identity_dir(parent: &Path) -> Result<std::path::PathBuf> {
    let stamp = base64url(&short_hash_16(b"tmp", now_ms().to_be_bytes().as_slice()));
    for attempt in 0..32u32 {
        let tmp_dir = parent.join(format!(
            ".tmp-identity-{}-{stamp}-{attempt}",
            std::process::id()
        ));
        match std::fs::create_dir(&tmp_dir) {
            Ok(()) => return Ok(tmp_dir),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("create temporary identity directory: {}", tmp_dir.display())
                });
            }
        }
    }
    anyhow::bail!(
        "create temporary identity directory in {} after 32 attempts",
        parent.display()
    )
}

fn derive_client_id(installation_id: &InstallationId) -> ClientId {
    ClientId(short_hash_16(
        b"brrmmmm-client-id-v1",
        installation_id.as_bytes(),
    ))
}

fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    atomic_write(path, data, FileMode::Private).map_err(anyhow::Error::new)
}

fn derive_key_id(public_key: &PublicKey) -> KeyId {
    let mut hasher = Sha256::new();
    hasher.update(b"brrmmmm-key-id-v1");
    hasher.update(public_key.as_bytes());
    let digest = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    KeyId(out)
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
    fn stale_public_key_file_is_repaired_automatically() {
        let dir = std::env::temp_dir().join(format!("brrmmmm-test-repair-{}", now_ms()));
        let identity = load_or_create_at(&dir).unwrap();
        let public_key_path = dir.join("public_key.bin");

        std::fs::write(&public_key_path, [0u8; 32]).unwrap();

        let repaired = load_or_create_at(&dir).unwrap();
        assert_eq!(repaired.public_key(), identity.public_key());
        assert_eq!(
            std::fs::read(&public_key_path).unwrap(),
            identity.public_key().as_bytes()
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn malformed_public_key_file_is_repaired_automatically() {
        let dir = std::env::temp_dir().join(format!("brrmmmm-test-malformed-key-{}", now_ms()));
        let identity = load_or_create_at(&dir).unwrap();
        let public_key_path = dir.join("public_key.bin");

        std::fs::write(&public_key_path, [7u8; 3]).unwrap();

        let repaired = load_or_create_at(&dir).unwrap();
        assert_eq!(repaired.public_key(), identity.public_key());
        assert_eq!(
            std::fs::read(&public_key_path).unwrap(),
            identity.public_key().as_bytes()
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn incomplete_identity_directory_is_corrupted_not_missing() {
        let dir = std::env::temp_dir().join(format!("brrmmmm-test-partial-{}", now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("identity.json"), b"{}").unwrap();

        let result = load_or_create_at(&dir);
        assert!(result.is_err());
        assert!(dir.join("identity.json").exists());

        let _ = std::fs::remove_dir_all(dir);
    }
}
