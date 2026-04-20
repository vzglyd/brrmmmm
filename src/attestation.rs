use std::collections::HashMap;
use std::str::FromStr;

use ed25519_dalek::{Signature, Signer, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};

use crate::identity::{ClientId, KeyId, PublicKey};
use crate::utils::base64url;

pub const PROTOCOL_VERSION: u8 = 1;
pub const UA_SHORT_HEX_CHARS: usize = 16;

pub const HEADER_VERSION: &str = "X-Brrm-Version";
pub const HEADER_CLIENT_ID: &str = "X-Brrm-Client-Id";
pub const HEADER_MISSION_ID: &str = "X-Brrm-Mission-Id";
pub const HEADER_MODULE_HASH: &str = "X-Brrm-Module-Hash";
pub const HEADER_REQUEST_COUNT: &str = "X-Brrm-Request-Count";
pub const HEADER_BEHAVIOR_HASH: &str = "X-Brrm-Behavior-Hash";
pub const HEADER_CAP_MASK: &str = "X-Brrm-Cap-Mask";
pub const HEADER_TIMESTAMP_MS: &str = "X-Brrm-Timestamp-Ms";
pub const HEADER_NONCE: &str = "X-Brrm-Nonce";
pub const HEADER_KEY_ID: &str = "X-Brrm-Key-Id";
pub const HEADER_PUBLIC_KEY: &str = "X-Brrm-Public-Key";
#[allow(dead_code)]
pub const HEADER_CREDENTIAL: &str = "X-Brrm-Credential";
pub const HEADER_CONTENT_DIGEST: &str = "X-Brrm-Content-Digest";
pub const HEADER_SIGNATURE: &str = "X-Brrm-Signature";

// ── Error type ───────────────────────────────────────────────────────

/// Typed errors from attestation operations, allowing callers to distinguish
/// malformed protocol (drop/log) from security violations (alert).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttestationError {
    /// A required header was absent from the request.
    MissingHeader(&'static str),
    /// A header value could not be parsed or had the wrong length.
    MalformedField(String),
    /// The public key or derived key_id did not match the trusted value.
    KeyMismatch,
    /// The content digest in the envelope did not match the request binding.
    ContentDigestMismatch,
    /// Ed25519 signature verification failed.
    SignatureInvalid,
    /// A binding field exceeds 65535 bytes and cannot be canonicalized safely.
    FieldTooLong,
}

impl std::fmt::Display for AttestationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingHeader(h) => write!(f, "missing header: {h}"),
            Self::MalformedField(msg) => write!(f, "malformed field: {msg}"),
            Self::KeyMismatch => write!(f, "key mismatch"),
            Self::ContentDigestMismatch => write!(f, "content digest mismatch"),
            Self::SignatureInvalid => write!(f, "signature invalid"),
            Self::FieldTooLong => {
                write!(f, "field too long for canonicalization (max 65535 bytes)")
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestBinding {
    pub method: String,
    pub authority: String,
    pub path: String,
    pub content_digest: Option<[u8; 32]>,
}

impl RequestBinding {
    pub fn new(
        method: impl Into<String>,
        authority: impl Into<String>,
        path: impl Into<String>,
        content_digest: Option<[u8; 32]>,
    ) -> Self {
        let path = normalize_signed_path(&path.into());
        Self {
            method: method.into().to_ascii_uppercase(),
            authority: authority.into().to_ascii_lowercase(),
            path,
            content_digest,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvelopeFields {
    pub client_id: ClientId,
    pub mission_id: [u8; 16],
    pub module_hash: [u8; 32],
    pub request_count: u64,
    pub behavior_hash: [u8; 16],
    pub cap_mask: u8,
    pub timestamp_ms: u64,
    pub nonce: [u8; 16],
    pub key_id: KeyId,
    pub public_key: PublicKey,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedEnvelope {
    pub user_agent_suffix: String,
    pub headers: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub struct VerifiedEnvelope {
    pub client_id: ClientId,
    pub mission_id: [u8; 16],
    pub module_hash: [u8; 32],
    pub request_count: u64,
    pub behavior_hash: [u8; 16],
    pub cap_mask: u8,
    pub timestamp_ms: u64,
    pub nonce: [u8; 16],
    pub key_id: KeyId,
}

pub fn build_signed_envelope<S>(
    fields: &EnvelopeFields,
    binding: &RequestBinding,
    signer: &S,
) -> Result<SignedEnvelope, AttestationError>
where
    S: Signer<ed25519_dalek::Signature>,
{
    let payload = canonical_payload(fields, binding)?;
    let signature = signer.sign(&payload);
    let signature_bytes = signature.to_bytes();
    let user_agent_suffix = user_agent_suffix(fields, binding.content_digest, &signature_bytes);
    let mut headers = vec![
        (HEADER_VERSION.to_string(), PROTOCOL_VERSION.to_string()),
        (HEADER_CLIENT_ID.to_string(), fields.client_id.to_string()),
        (HEADER_MISSION_ID.to_string(), base64url(&fields.mission_id)),
        (
            HEADER_MODULE_HASH.to_string(),
            base64url(&fields.module_hash),
        ),
        (
            HEADER_REQUEST_COUNT.to_string(),
            fields.request_count.to_string(),
        ),
        (
            HEADER_BEHAVIOR_HASH.to_string(),
            base64url(&fields.behavior_hash),
        ),
        (
            HEADER_CAP_MASK.to_string(),
            format!("{:02x}", fields.cap_mask),
        ),
        (
            HEADER_TIMESTAMP_MS.to_string(),
            fields.timestamp_ms.to_string(),
        ),
        (HEADER_NONCE.to_string(), base64url(&fields.nonce)),
        (HEADER_KEY_ID.to_string(), fields.key_id.to_string()),
        (HEADER_PUBLIC_KEY.to_string(), fields.public_key.to_string()),
    ];
    if let Some(content_digest) = binding.content_digest {
        headers.push((
            HEADER_CONTENT_DIGEST.to_string(),
            base64url(&content_digest),
        ));
    }
    headers.push((HEADER_SIGNATURE.to_string(), base64url(&signature_bytes)));
    Ok(SignedEnvelope {
        user_agent_suffix,
        headers,
    })
}

pub fn user_agent_suffix(
    fields: &EnvelopeFields,
    content_digest: Option<[u8; 32]>,
    signature: &[u8; 64],
) -> String {
    let mut parts = vec![
        format!("brrm/{PROTOCOL_VERSION}"),
        format!("cid/{}", short_hex(fields.client_id.as_bytes())),
        format!("mid/{}", short_hex(&fields.mission_id)),
        format!("mod/{}", short_hex(&fields.module_hash)),
        format!("seq/{}", fields.request_count),
        format!("cap/{}", cap_names(fields.cap_mask)),
        format!("bh/{}", short_hex(&fields.behavior_hash)),
        format!("ts/{}", ua_timestamp(fields.timestamp_ms)),
        format!("nonce/{}", base64url(&fields.nonce)),
        format!("kid/{}", short_hex(fields.key_id.as_bytes())),
        format!("pk/{}", fields.public_key),
    ];
    if let Some(content_digest) = content_digest {
        parts.push(format!("cd/{}", short_hex(&content_digest)));
    }
    parts.push(format!("sig/{}", base64url(signature)));
    parts.join(" ")
}

#[allow(dead_code)]
pub fn verify_signed_envelope(
    headers: &[(String, String)],
    binding: &RequestBinding,
    trusted_public_key: Option<PublicKey>,
) -> Result<VerifiedEnvelope, AttestationError> {
    let map = header_map(headers);
    let version = required(&map, HEADER_VERSION)?;
    if version != PROTOCOL_VERSION.to_string() {
        return Err(AttestationError::MalformedField(format!(
            "unsupported brrm version {version}"
        )));
    }

    let client_id = ClientId::from_str(required(&map, HEADER_CLIENT_ID)?)
        .map_err(|e| AttestationError::MalformedField(e.to_string()))?;
    let mission_id = parse_raw::<16>(required(&map, HEADER_MISSION_ID)?)?;
    let module_hash = parse_raw::<32>(required(&map, HEADER_MODULE_HASH)?)?;
    let request_count = required(&map, HEADER_REQUEST_COUNT)?
        .parse::<u64>()
        .map_err(|e| AttestationError::MalformedField(format!("request count parse: {e}")))?;
    let behavior_hash = parse_raw::<16>(required(&map, HEADER_BEHAVIOR_HASH)?)?;
    let cap_mask = u8::from_str_radix(required(&map, HEADER_CAP_MASK)?, 16)
        .map_err(|e| AttestationError::MalformedField(format!("cap mask parse: {e}")))?;
    let timestamp_ms = required(&map, HEADER_TIMESTAMP_MS)?
        .parse::<u64>()
        .map_err(|e| AttestationError::MalformedField(format!("timestamp parse: {e}")))?;
    let nonce = parse_raw::<16>(required(&map, HEADER_NONCE)?)?;
    let key_id = KeyId::from_str(required(&map, HEADER_KEY_ID)?)
        .map_err(|e| AttestationError::MalformedField(e.to_string()))?;
    let public_key = PublicKey::from_str(required(&map, HEADER_PUBLIC_KEY)?)
        .map_err(|e| AttestationError::MalformedField(e.to_string()))?;

    if let Some(expected) = trusted_public_key
        && public_key != expected
    {
        return Err(AttestationError::KeyMismatch);
    }
    let expected_key_id = derive_key_id(&public_key);
    if key_id != expected_key_id {
        return Err(AttestationError::KeyMismatch);
    }

    let content_digest = map
        .get(&HEADER_CONTENT_DIGEST.to_ascii_lowercase())
        .map(|value| parse_raw::<32>(value))
        .transpose()?;
    if content_digest != binding.content_digest {
        return Err(AttestationError::ContentDigestMismatch);
    }

    let fields = EnvelopeFields {
        client_id,
        mission_id,
        module_hash,
        request_count,
        behavior_hash,
        cap_mask,
        timestamp_ms,
        nonce,
        key_id,
        public_key,
    };
    let payload = canonical_payload(&fields, binding)?;
    let signature = parse_raw::<64>(required(&map, HEADER_SIGNATURE)?)?;
    let signature = Signature::from_bytes(&signature);
    let verifying_key = VerifyingKey::from_bytes(public_key.as_bytes())
        .map_err(|_| AttestationError::KeyMismatch)?;
    verifying_key
        .verify(&payload, &signature)
        .map_err(|_| AttestationError::SignatureInvalid)?;

    Ok(VerifiedEnvelope {
        client_id,
        mission_id,
        module_hash,
        request_count,
        behavior_hash,
        cap_mask,
        timestamp_ms,
        nonce,
        key_id,
    })
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

pub fn binding_from_url(
    method: impl Into<String>,
    url: &str,
    content_digest: Option<[u8; 32]>,
) -> Option<RequestBinding> {
    let parsed = reqwest::Url::parse(url).ok()?;
    match parsed.scheme() {
        "http" | "https" => {}
        _ => return None,
    }
    let host = parsed.host_str()?;
    let authority = match parsed.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    };
    let mut path = parsed.path().to_string();
    if let Some(query) = parsed.query() {
        path.push('?');
        path.push_str(query);
    }
    Some(RequestBinding::new(method, authority, path, content_digest))
}

pub fn is_reserved_header(name: &str) -> bool {
    name.eq_ignore_ascii_case("user-agent") || name.to_ascii_lowercase().starts_with("x-brrm-")
}

pub fn merge_host_headers<I>(
    original: I,
    user_agent: &str,
    envelope: Option<&SignedEnvelope>,
) -> Vec<(String, String)>
where
    I: IntoIterator<Item = (String, String)>,
{
    let mut headers = Vec::new();
    for (name, value) in original {
        if is_reserved_header(&name) {
            continue;
        }
        headers.push((name, value));
    }
    headers.push(("User-Agent".to_string(), user_agent.to_string()));
    if let Some(envelope) = envelope {
        headers.extend(envelope.headers.iter().cloned());
    }
    headers
}

fn short_hex(bytes: &[u8]) -> String {
    hex_prefix(bytes, UA_SHORT_HEX_CHARS)
}

fn hex_prefix(bytes: &[u8], chars: usize) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(chars);
    for byte in bytes {
        if out.len() >= chars {
            break;
        }
        out.push(HEX[(byte >> 4) as usize] as char);
        if out.len() >= chars {
            break;
        }
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn cap_names(mask: u8) -> String {
    let mut names = Vec::new();
    if mask & 0x01 != 0 {
        names.push("network");
    }
    if mask & 0x02 != 0 {
        names.push("browser");
    }
    if mask & 0x04 != 0 {
        names.push("ai");
    }
    if mask & 0x08 != 0 {
        names.push("kv");
    }
    if names.is_empty() {
        "none".to_string()
    } else {
        names.join("+")
    }
}

fn ua_timestamp(ms: u64) -> String {
    let secs = ms / 1000;
    let millis = ms % 1000;
    let (y, mo, d) = civil_from_days((secs / 86400) as i64);
    let h = (secs / 3600) % 24;
    let m = (secs / 60) % 60;
    let s = secs % 60;
    format!("{y:04}{mo:02}{d:02}T{h:02}{m:02}{s:02}.{millis:03}Z")
}

/// Howard Hinnant's civil calendar algorithm.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

/// Constructs the canonical byte payload that is signed or verified.
/// Returns `Err(AttestationError::FieldTooLong)` if any binding string field
/// exceeds 65535 bytes, which would cause a silent mismatch if truncated.
pub(crate) fn canonical_payload(
    fields: &EnvelopeFields,
    binding: &RequestBinding,
) -> Result<Vec<u8>, AttestationError> {
    let mut out = Vec::with_capacity(
        180 + binding.method.len() + binding.authority.len() + binding.path.len(),
    );
    out.push(PROTOCOL_VERSION);
    out.extend_from_slice(fields.client_id.as_bytes());
    out.extend_from_slice(&fields.mission_id);
    out.extend_from_slice(&fields.module_hash);
    out.extend_from_slice(&fields.request_count.to_be_bytes());
    out.extend_from_slice(&fields.behavior_hash);
    out.push(fields.cap_mask);
    out.extend_from_slice(&fields.timestamp_ms.to_be_bytes());
    out.extend_from_slice(&fields.nonce);
    push_string(&mut out, &binding.method)?;
    push_string(&mut out, &binding.authority)?;
    push_string(&mut out, &binding.path)?;
    match binding.content_digest {
        Some(digest) => {
            out.push(1);
            out.extend_from_slice(&digest);
        }
        None => out.push(0),
    }
    Ok(out)
}

/// Encodes a length-prefixed string into the canonical payload buffer.
/// Returns `Err(AttestationError::FieldTooLong)` rather than silently truncating,
/// since truncation would produce a different canonical form than what the sidecar signed.
fn push_string(out: &mut Vec<u8>, value: &str) -> Result<(), AttestationError> {
    let bytes = value.as_bytes();
    let len = u16::try_from(bytes.len()).map_err(|_| AttestationError::FieldTooLong)?;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(bytes);
    Ok(())
}

fn normalize_signed_path(path: &str) -> String {
    if path.is_empty() {
        "/".to_string()
    } else if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    }
}

#[allow(dead_code)]
fn header_map(headers: &[(String, String)]) -> HashMap<String, String> {
    headers
        .iter()
        .map(|(name, value)| (name.to_ascii_lowercase(), value.clone()))
        .collect()
}

#[allow(dead_code)]
fn required<'a>(
    headers: &'a HashMap<String, String>,
    name: &'static str,
) -> Result<&'a str, AttestationError> {
    headers
        .get(&name.to_ascii_lowercase())
        .map(|value| value.as_str())
        .ok_or(AttestationError::MissingHeader(name))
}

fn parse_raw<const N: usize>(value: &str) -> Result<[u8; N], AttestationError> {
    let bytes = crate::utils::decode_base64url(value).map_err(AttestationError::MalformedField)?;
    bytes.try_into().map_err(|bytes: Vec<u8>| {
        AttestationError::MalformedField(format!("expected {N} bytes, got {}", bytes.len()))
    })
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::SigningKey;

    use super::*;

    fn key() -> SigningKey {
        SigningKey::from_bytes(&[7u8; 32])
    }

    fn fields(public_key: PublicKey) -> EnvelopeFields {
        EnvelopeFields {
            client_id: ClientId([1u8; 16]),
            mission_id: [2u8; 16],
            module_hash: [3u8; 32],
            request_count: 42,
            behavior_hash: [4u8; 16],
            cap_mask: 0x05,
            timestamp_ms: 123_456_789,
            nonce: [5u8; 16],
            key_id: derive_key_id(&public_key),
            public_key,
        }
    }

    #[test]
    fn signed_envelope_verifies() {
        let key = key();
        let public_key = PublicKey(key.verifying_key().to_bytes());
        let fields = fields(public_key);
        let binding = RequestBinding::new("GET", "Example.COM", "/v1?q=1", None);
        let envelope = build_signed_envelope(&fields, &binding, &key).unwrap();

        let verified =
            verify_signed_envelope(&envelope.headers, &binding, Some(public_key)).unwrap();

        assert_eq!(verified.client_id, fields.client_id);
        assert_eq!(verified.request_count, 42);
        assert_eq!(verified.cap_mask, 0x05);
    }

    #[test]
    fn tampered_binding_fails_verification() {
        let key = key();
        let public_key = PublicKey(key.verifying_key().to_bytes());
        let fields = fields(public_key);
        let binding = RequestBinding::new("GET", "example.com", "/v1", None);
        let envelope = build_signed_envelope(&fields, &binding, &key).unwrap();
        let tampered = RequestBinding::new("GET", "example.com", "/v2", None);

        assert!(verify_signed_envelope(&envelope.headers, &tampered, Some(public_key)).is_err());
    }

    #[test]
    fn user_agent_suffix_is_readable_and_stable() {
        let key = key();
        let public_key = PublicKey(key.verifying_key().to_bytes());
        let mut fields = fields(public_key);
        fields.timestamp_ms = 123_456_789;
        let binding = RequestBinding::new("POST", "example.com", "/v1", Some([9u8; 32]));
        let envelope = build_signed_envelope(&fields, &binding, &key).unwrap();

        let ua = envelope.user_agent_suffix;
        assert!(ua.contains("brrm/1"));
        assert!(ua.contains("cid/0101010101010101"));
        assert!(ua.contains("mid/0202020202020202"));
        assert!(ua.contains("mod/0303030303030303"));
        assert!(ua.contains("seq/42"));
        assert!(ua.contains("cap/network+ai"));
        assert!(ua.contains("bh/0404040404040404"));
        assert!(ua.contains("ts/19700102T101736.789Z"));
        assert!(ua.contains("nonce/BQUFBQUFBQUFBQUFBQUFBQ"));
        assert!(ua.contains("cd/0909090909090909"));
        assert!(ua.contains("pk/"));
        assert!(ua.contains("sig/"));
    }

    #[test]
    fn reserved_headers_are_removed_before_host_headers_are_appended() {
        let merged = merge_host_headers(
            vec![
                ("Accept".to_string(), "application/json".to_string()),
                ("User-Agent".to_string(), "sidecar".to_string()),
                ("X-Brrm-Signature".to_string(), "fake".to_string()),
            ],
            "host",
            None,
        );

        assert_eq!(
            merged,
            vec![
                ("Accept".to_string(), "application/json".to_string()),
                ("User-Agent".to_string(), "host".to_string()),
            ]
        );
    }

    #[test]
    fn empty_path_normalizes_to_slash() {
        let binding = RequestBinding::new("GET", "example.com", "", None);
        assert_eq!(binding.path, "/");
        // Verify it still canonicalizes correctly (no FieldTooLong)
        let key = key();
        let public_key = PublicKey(key.verifying_key().to_bytes());
        let fields = fields(public_key);
        assert!(canonical_payload(&fields, &binding).is_ok());
    }

    #[test]
    fn special_characters_in_path_are_handled() {
        let binding =
            RequestBinding::new("GET", "example.com", "/search?q=hello+world&lang=en", None);
        let key = key();
        let public_key = PublicKey(key.verifying_key().to_bytes());
        let fields = fields(public_key);
        let envelope = build_signed_envelope(&fields, &binding, &key).unwrap();
        assert!(verify_signed_envelope(&envelope.headers, &binding, Some(public_key)).is_ok());
    }

    #[test]
    fn oversized_field_returns_field_too_long_error() {
        let long_path = "/".repeat(70_000); // > u16::MAX
        let binding = RequestBinding::new("GET", "example.com", long_path, None);
        let key = key();
        let public_key = PublicKey(key.verifying_key().to_bytes());
        let fields = fields(public_key);
        let result = build_signed_envelope(&fields, &binding, &key);
        assert_eq!(result, Err(AttestationError::FieldTooLong));
    }

    #[test]
    fn user_agent_suffix_is_deterministic_across_calls() {
        let key = key();
        let public_key = PublicKey(key.verifying_key().to_bytes());
        let fields = fields(public_key);
        let binding = RequestBinding::new("GET", "example.com", "/v1", None);
        // Use the same nonce/timestamp — build_signed_envelope signs deterministically
        // for identical inputs (same signing key, same payload).
        let payload = canonical_payload(&fields, &binding).unwrap();
        let sig1 = key.sign(&payload).to_bytes();
        let sig2 = key.sign(&payload).to_bytes();
        let ua1 = user_agent_suffix(&fields, None, &sig1);
        let ua2 = user_agent_suffix(&fields, None, &sig2);
        assert_eq!(ua1, ua2);
    }
}
