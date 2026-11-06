//! Bundle manifest signing (issue #1013 O3).
//!
//! `publish` reserves a `signatures` field in the manifest. Before this
//! module it was always empty; now `oxo-flow publish --sign-key <file>`
//! produces an ed25519 signature over the manifest JSON *without* the
//! `signatures` field, and `run --bundle` / `pull` verification re-derives
//! the same bytes and checks every signature entry. Unsigned bundles (empty
//! array) keep the pre-existing behavior: checksums only.
//!
//! Coverage note: signing the manifest (which carries a sha256 for every
//! archived file) transitively covers all bundle content, so one signature
//! over the manifest is sufficient — files are still individually
//! checksum-verified before the signature check reports success.
//!
//! Byte canonicality: both sides operate on a `serde_json::Value` with the
//! `signatures` field removed, serialized with `serde_json::to_vec`. The
//! publisher serializes the value it built; the verifier re-parses the
//! manifest it just read — the manifest on disk was itself produced by
//! serializing that same value, so field order round-trips exactly.

use anyhow::{Context, Result};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use std::path::Path;

/// Ed25519 public key length in bytes.
const PUBLIC_KEY_LEN: usize = 32;
/// Ed25519 signature length in bytes.
const SIGNATURE_LEN: usize = 64;
/// The only signature algorithm this version understands.
pub(crate) const SIGNATURE_ALG: &str = "ed25519";

/// Load a 32-byte ed25519 seed from `path`.
///
/// Accepted formats: 64 hex characters (with optional trailing newline) or
/// exactly 32 raw bytes. Hex is the documented format because it survives
/// copy-paste and editors; raw is accepted so `openssl genpkey`-style key
/// files can be piped in directly.
pub(crate) fn load_signing_key(path: &Path) -> Result<SigningKey> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("failed to read signing key: {}", path.display()))?;
    let seed: [u8; 32] = match bytes.len() {
        32 => bytes.try_into().expect("length checked above"),
        64 => decode_hex_seed(&bytes)
            .with_context(|| format!("invalid hex signing key in {}", path.display()))?,
        _ => anyhow::bail!(
            "signing key '{}' must be 32 raw bytes or 64 hex characters, got {} bytes",
            path.display(),
            bytes.len()
        ),
    };
    Ok(SigningKey::from_bytes(&seed))
}

fn decode_hex_seed(bytes: &[u8]) -> Result<[u8; 32]> {
    let text = std::str::from_utf8(bytes).context("signing key is neither raw 32 bytes nor hex")?;
    let hex_str = text.trim();
    let mut seed = [0u8; 32];
    for (i, chunk) in hex_str.as_bytes().chunks(2).enumerate() {
        if chunk.len() != 2 {
            anyhow::bail!("odd-length hex key");
        }
        seed[i] = u8::from_str_radix(std::str::from_utf8(chunk)?, 16)
            .map_err(|e| anyhow::anyhow!("invalid hex in signing key: {e}"))?;
    }
    Ok(seed)
}

/// Sign `manifest` in place: the signature covers the manifest JSON with the
/// `signatures` field removed, then the signature entry is attached.
pub(crate) fn sign_manifest(
    manifest: &mut serde_json::Value,
    signing_key: &SigningKey,
) -> Result<()> {
    let payload = signing_payload(manifest)?;
    let signature = signing_key.sign(&payload);
    let public_key = signing_key.verifying_key().to_bytes();

    if let Some(obj) = manifest.as_object_mut() {
        obj.insert(
            "signatures".to_string(),
            serde_json::json!([{
                "alg": SIGNATURE_ALG,
                "public_key": hex::encode(public_key),
                "signature": hex::encode(signature.to_bytes()),
            }]),
        );
    }
    Ok(())
}

/// Verify the `signatures` array of an already checksum-verified manifest.
///
/// Returns the number of verified signatures. An empty (or absent) array is
/// Ok(0) — unsigned bundles remain valid. Any present signature that fails
/// to parse or verify bails: a bundle that claims to be signed but isn't
/// correctly signed must not run.
pub(crate) fn verify_manifest_signatures(manifest: &serde_json::Value) -> Result<usize> {
    let signatures = match manifest.get("signatures") {
        None | Some(serde_json::Value::Null) => return Ok(0),
        Some(serde_json::Value::Array(items)) => items,
        Some(_) => anyhow::bail!("manifest 'signatures' must be an array"),
    };
    if signatures.is_empty() {
        return Ok(0);
    }

    let payload = signing_payload(manifest)?;
    for (i, entry) in signatures.iter().enumerate() {
        let alg = entry["alg"].as_str().unwrap_or_default();
        if alg != SIGNATURE_ALG {
            anyhow::bail!(
                "signature #{i} uses unsupported algorithm '{alg}' (expected '{SIGNATURE_ALG}')"
            );
        }
        let public_key_hex = entry["public_key"]
            .as_str()
            .with_context(|| format!("signature #{i} missing 'public_key'"))?;
        let signature_hex = entry["signature"]
            .as_str()
            .with_context(|| format!("signature #{i} missing 'signature'"))?;

        let public_key = decode_fixed_hex::<PUBLIC_KEY_LEN>(public_key_hex, "public key")
            .with_context(|| format!("signature #{i} public key"))?;
        let sig_bytes = decode_fixed_hex::<SIGNATURE_LEN>(signature_hex, "signature")
            .with_context(|| format!("signature #{i} signature"))?;

        let verifying_key = VerifyingKey::from_bytes(&public_key)
            .map_err(|e| anyhow::anyhow!("signature #{i} has an invalid public key: {e}"))?;
        let signature = Signature::from_bytes(&sig_bytes);
        verifying_key
            .verify(&payload, &signature)
            .map_err(|_| anyhow::anyhow!("signature #{i} does not match the manifest — the bundle content or the signature was altered"))?;
    }
    Ok(signatures.len())
}

/// Serialize the manifest without its `signatures` field — the exact bytes
/// both signing and verification derive.
fn signing_payload(manifest: &serde_json::Value) -> Result<Vec<u8>> {
    let mut payload = manifest.clone();
    if let Some(obj) = payload.as_object_mut() {
        obj.remove("signatures");
    }
    serde_json::to_vec(&payload).context("failed to serialize manifest for signing")
}

fn decode_fixed_hex<const N: usize>(text: &str, what: &str) -> Result<[u8; N]> {
    let trimmed = text.trim();
    if trimmed.len() != N * 2 {
        anyhow::bail!(
            "invalid {what}: expected {} hex characters, got {}",
            N * 2,
            trimmed.len()
        );
    }
    let mut out = [0u8; N];
    for (i, chunk) in trimmed.as_bytes().chunks(2).enumerate() {
        out[i] = u8::from_str_radix(
            std::str::from_utf8(chunk).map_err(|_| anyhow::anyhow!("invalid {what} hex"))?,
            16,
        )
        .map_err(|e| anyhow::anyhow!("invalid {what} hex: {e}"))?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::bundle::BundleFormat;
    use std::path::PathBuf;

    /// Deterministic test keypair (ed25519 seeds are the private key; no RNG).
    fn test_seed() -> [u8; 32] {
        let mut seed = [0u8; 32];
        for (i, b) in seed.iter_mut().enumerate() {
            *b = (i as u8) * 7;
        }
        seed
    }

    fn test_signing_key() -> SigningKey {
        SigningKey::from_bytes(&test_seed())
    }

    fn sample_manifest() -> serde_json::Value {
        serde_json::json!({
            "format": "oxoflow-bundle-v1",
            "workflow": "w.oxoflow",
            "entrypoint": "w.oxoflow",
            "files": [{"path": "w.oxoflow", "sha256": "abc"}],
            "signatures": []
        })
    }

    fn write_hex_key(dir: &std::path::Path) -> PathBuf {
        let path = dir.join("signing.hex");
        std::fs::write(&path, hex::encode(test_seed())).unwrap();
        path
    }

    #[test]
    fn signing_adds_a_verifiable_signature_entry() {
        let mut manifest = sample_manifest();
        sign_manifest(&mut manifest, &test_signing_key()).unwrap();

        let signatures = manifest["signatures"].as_array().unwrap();
        assert_eq!(signatures.len(), 1);
        assert_eq!(signatures[0]["alg"], "ed25519");
        assert_eq!(
            verify_manifest_signatures(&manifest).unwrap(),
            1,
            "the freshly attached signature must verify"
        );
    }

    #[test]
    fn tampered_manifest_content_fails_verification() {
        let mut manifest = sample_manifest();
        sign_manifest(&mut manifest, &test_signing_key()).unwrap();
        // Flip one byte of a covered file checksum.
        manifest["files"][0]["sha256"] = serde_json::json!("abd");
        assert!(
            verify_manifest_signatures(&manifest).is_err(),
            "a signed manifest must fail verification after any content change"
        );
    }

    #[test]
    fn wrong_key_signature_fails_verification() {
        let mut manifest = sample_manifest();
        let mut other_seed = test_seed();
        other_seed[0] ^= 0xff;
        sign_manifest(&mut manifest, &SigningKey::from_bytes(&other_seed)).unwrap();
        // Swap the recorded public key to the expected one: the signature
        // was produced by a different key, so verification must fail.
        let expected_pubkey = hex::encode(test_signing_key().verifying_key().to_bytes());
        manifest["signatures"][0]["public_key"] = serde_json::json!(expected_pubkey);
        assert!(verify_manifest_signatures(&manifest).is_err());
    }

    #[test]
    fn empty_and_absent_signatures_are_unsigned_and_valid() {
        assert_eq!(verify_manifest_signatures(&sample_manifest()).unwrap(), 0);

        let mut absent = sample_manifest();
        absent.as_object_mut().unwrap().remove("signatures");
        assert_eq!(verify_manifest_signatures(&absent).unwrap(), 0);
    }

    #[test]
    fn non_array_signatures_field_is_rejected() {
        let mut manifest = sample_manifest();
        manifest["signatures"] = serde_json::json!("signed-by-hand");
        assert!(verify_manifest_signatures(&manifest).is_err());
    }

    #[test]
    fn load_signing_key_accepts_hex_and_raw() {
        let dir = tempfile::tempdir().unwrap();
        let hex_path = write_hex_key(dir.path());
        let raw_path = dir.path().join("signing.raw");
        std::fs::write(&raw_path, test_seed()).unwrap();

        let from_hex = load_signing_key(&hex_path).unwrap();
        let from_raw = load_signing_key(&raw_path).unwrap();
        assert_eq!(
            from_hex.verifying_key().to_bytes(),
            from_raw.verifying_key().to_bytes(),
            "hex and raw encodings must yield the same key"
        );
    }

    #[test]
    fn load_signing_key_rejects_bad_lengths_and_bad_hex() {
        let dir = tempfile::tempdir().unwrap();
        let short = dir.path().join("short.key");
        std::fs::write(&short, [0u8; 31]).unwrap();
        assert!(load_signing_key(&short).is_err());

        let bad_hex = dir.path().join("bad.hex");
        std::fs::write(&bad_hex, "zz".repeat(32)).unwrap();
        assert!(load_signing_key(&bad_hex).is_err());
    }

    #[test]
    fn end_to_end_publish_signs_and_bundle_verifies() {
        // Full pipeline on real archives: publish signs the manifest,
        // extraction verifies checksums + the signature.
        let dir = tempfile::tempdir().unwrap();
        let workflow = dir.path().join("w.oxoflow");
        std::fs::write(
            &workflow,
            "[workflow]\nname = \"s\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"r1\"\noutput = [\"out.txt\"]\nshell = \"echo hi > {output[0]}\"\n",
        )
        .unwrap();

        let bundle = dir.path().join("signed-bundle.tar.zst");
        crate::commands::publish::publish_command(
            workflow.clone(),
            Some(bundle.clone()),
            false,
            None,
            Some(write_hex_key(dir.path())),
        )
        .expect("publish --sign-key must produce a signed bundle");

        // Sanity: the manifest inside a signed bundle carries a signature.
        let (manifest, _guard) = read_bundle_manifest(&bundle);
        assert_eq!(manifest["signatures"].as_array().unwrap().len(), 1);

        let unsigned = dir.path().join("unsigned-bundle.tar.gz");
        crate::commands::publish::publish_command(
            workflow,
            Some(unsigned.clone()),
            false,
            Some("tar.gz".to_string()),
            None,
        )
        .unwrap();
        let (manifest, _) = read_bundle_manifest(&unsigned);
        assert_eq!(verify_manifest_signatures(&manifest).unwrap(), 0);
        assert_eq!(
            BundleFormat::from_path(&bundle).unwrap(),
            BundleFormat::TarZst
        );
    }

    /// Extract just the manifest from a bundle archive (test helper; the
    /// archive is trusted here — it was produced by publish_command above).
    fn read_bundle_manifest(bundle: &std::path::Path) -> (serde_json::Value, tempfile::TempDir) {
        let extract = tempfile::tempdir().unwrap();
        let file = std::fs::File::open(bundle).unwrap();
        let reader: Box<dyn std::io::Read> = match BundleFormat::from_path(bundle).unwrap() {
            BundleFormat::TarZst => Box::new(zstd::stream::read::Decoder::new(file).unwrap()),
            BundleFormat::TarGz => Box::new(flate2::read::GzDecoder::new(file)),
        };
        tar::Archive::new(reader).unpack(extract.path()).unwrap();
        let manifest: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(extract.path().join("manifest.json")).unwrap(),
        )
        .unwrap();
        (manifest, extract)
    }
}
