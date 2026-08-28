//! At-rest protection for secrets stored in the local database
//! (issue #205): third-party AI provider API keys.
//!
//! `OXO_FLOW_MASTER_KEY` seeds an AES-256-GCM key (SHA-256 of the secret).
//! With the var unset, writes stay plaintext (previous behavior) and reads
//! accept both forms, so enabling or rotating is transparent and nothing
//! bricks when the secret is missing.

use aes_gcm::aead::rand_core::RngCore;
use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use sha2::{Digest, Sha256};

const PREFIX: &str = "v1:";

fn master_key() -> Option<[u8; 32]> {
    let raw = std::env::var("OXO_FLOW_MASTER_KEY").ok()?;
    if raw.is_empty() {
        return None;
    }
    Some(Sha256::digest(raw.as_bytes()).into())
}

fn encrypt_with(key: &[u8; 32], plain: &str) -> String {
    let cipher = Aes256Gcm::new(key.into());
    let mut nonce_bytes = [0u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);
    let ct = cipher
        .encrypt(Nonce::from_slice(&nonce_bytes), plain.as_bytes())
        .expect("AES-GCM encryption cannot fail for valid inputs");
    format!("{PREFIX}{}:{}", B64.encode(nonce_bytes), B64.encode(ct))
}

fn decrypt_with(key: &[u8; 32], stored: &str) -> Option<String> {
    let body = stored.strip_prefix(PREFIX)?;
    let (nonce_b64, ct_b64) = body.split_once(':')?;
    let nonce = B64.decode(nonce_b64).ok()?;
    let ct = B64.decode(ct_b64).ok()?;
    let cipher = Aes256Gcm::new(key.into());
    cipher
        .decrypt(Nonce::from_slice(&nonce), ct.as_ref())
        .ok()
        .and_then(|plain| String::from_utf8(plain).ok())
}

/// Encrypt to the stored format, or return plaintext unchanged when no
/// master key is configured (legacy mode).
pub fn seal(plain: &str) -> String {
    match master_key() {
        Some(key) => encrypt_with(&key, plain),
        None => plain.to_string(),
    }
}

/// Decrypt anything previously written: transparently passes legacy
/// plaintext through; `v1:` payloads require the same master key.
///
/// An unreadable payload degrades to an empty string — callers that need to
/// distinguish "no key stored" from "key stored but unreadable" must consult
/// [`is_recoverable`] instead of testing the result for emptiness.
pub fn open(stored: &str) -> String {
    if !stored.starts_with(PREFIX) {
        return stored.to_string();
    }
    let Some(key) = master_key() else {
        tracing::error!("Encrypted AI credential present but OXO_FLOW_MASTER_KEY unset");
        return String::new();
    };
    match decrypt_with(&key, stored) {
        Some(plain) => plain,
        None => {
            tracing::error!(
                "AI credential decryption failed (wrong OXO_FLOW_MASTER_KEY or corrupt row)"
            );
            String::new()
        }
    }
}

/// Whether at-rest encryption is active (`seal` writes `v1:` ciphertext).
/// Exposed so `/api/health` can surface plaintext key storage to API
/// consumers, not only to whoever reads the server log.
pub fn master_key_configured() -> bool {
    master_key().is_some()
}

/// Loud startup notice when credentials would be written unencrypted. Shared
/// by both entry points (the standalone `oxo-flow-web` binary and
/// `oxo-flow serve`) so neither deployment path can skip it silently.
pub fn warn_if_plaintext_key() {
    if master_key().is_none() {
        tracing::warn!(
            "OXO_FLOW_MASTER_KEY is not set: AI provider keys are stored as \
             plaintext in the local database. Set it to encrypt new writes \
             (existing rows remain readable). /api/health reports the live \
             state as components.ai_key_storage."
        );
    }
}

/// Whether `stored` can be read back under the current configuration: legacy
/// plaintext always can, a `v1:` payload needs the same master key. This is
/// the "is the credential usable" test — a rotated master key leaves the row
/// in place while [`open`] degrades it to an empty string.
pub fn is_recoverable(stored: &str) -> bool {
    recoverable_with(master_key().as_ref(), stored)
}

/// Pure core of [`is_recoverable`] — the key is injected so tests need no
/// environment mutation.
fn recoverable_with(key: Option<&[u8; 32]>, stored: &str) -> bool {
    if !stored.starts_with(PREFIX) {
        return true;
    }
    match key {
        Some(key) => decrypt_with(key, stored).is_some(),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const K1: [u8; 32] = [7u8; 32];
    const K2: [u8; 32] = [9u8; 32];

    #[test]
    fn sealed_form_is_prefixed_and_roundtrips_only_with_right_key() {
        let sealed = encrypt_with(&K1, "sk-live-abc123");
        assert!(sealed.starts_with(PREFIX));
        assert!(!sealed.contains("sk-live"));
        assert_eq!(
            decrypt_with(&K1, &sealed).as_deref(),
            Some("sk-live-abc123")
        );
        assert_eq!(decrypt_with(&K2, &sealed), None);
        // Fresh random nonce per write: identical plaintext must differ.
        assert_ne!(encrypt_with(&K1, "x"), encrypt_with(&K1, "x"));
    }

    #[test]
    fn legacy_plaintext_passes_through_when_no_master_key() {
        // This test binary never sets OXO_FLOW_MASTER_KEY; if that ever
        // changes, seal() here would emit a v1: row and this catches it.
        assert_eq!(open("plaintext-key"), "plaintext-key");
        assert_eq!(seal("plaintext-key"), "plaintext-key");
    }

    #[test]
    fn pair_semantics_match_wrapper_contract() {
        // The wrappers add only key lookup + passthrough on top of these
        // primitives — pin that contract down without mutating process env.
        let sealed = encrypt_with(&K1, "row-key");
        assert_eq!(decrypt_with(&K2, &sealed), None);
        assert_eq!(decrypt_with(&K1, &sealed).as_deref(), Some("row-key"));
        assert!(seal("whatever") == "whatever" || seal("whatever").starts_with(PREFIX));
    }

    #[test]
    fn recoverability_tracks_the_available_master_key() {
        // Legacy plaintext is always readable, sealed ciphertext only under
        // its own key — a rotated key leaves the row in place but unusable.
        assert!(recoverable_with(None, "plaintext-key"));
        assert!(!recoverable_with(None, "v1:bm9uY2U6Y2lwaGVydGV4dA"));
        assert!(recoverable_with(Some(&K1), &encrypt_with(&K1, "row-key")));
        assert!(!recoverable_with(Some(&K2), &encrypt_with(&K1, "row-key")));
        // Garbage that merely claims the sealed prefix is not recoverable.
        assert!(!recoverable_with(Some(&K1), "v1:not-base64:not-base64"));
    }

    #[test]
    fn key_presence_flag_matches_the_wrapper_contract() {
        // This test binary never sets OXO_FLOW_MASTER_KEY (see the plaintext
        // test above); the health endpoint surfaces this flag verbatim.
        assert!(!master_key_configured());
        assert_eq!(seal("x"), "x");
    }
}
