//! The signature that makes a downloaded archive trustworthy (decision 0054).
//!
//! The release workflow signs, with an Ed25519 key whose private half only the human holds, this
//! message for each archive:
//!
//! ```text
//! acs-release-v1\n<tag>\n<archive file name>\n<lowercase hex SHA-256 of the archive>\n
//! ```
//!
//! Four lines, **each ended by a newline, the last one too** (the workflow builds it with
//! `printf 'acs-release-v1\n%s\n%s\n%s\n'`, in `.github/sign-release-message.sh`; a test keeps
//! the two in step). It binds the version and the file name to the archive's bytes, so an old
//! signed archive cannot be passed off as a newer release. The `.sig` file holds the 64-byte
//! signature in base64 on one line.

use ed25519_compact::{PublicKey, Signature};
use sha2::{Digest, Sha256};

/// The release public key compiled into this build (32 bytes).
///
/// All zeros means that no key was created yet: every update is refused with "no release key in
/// this build". The human replaces it with the public half of the key stored in the secret
/// `ACS_RELEASE_SIGNING_KEY` (the commands are in `docs/releases/README.md`).
pub const RELEASE_PUBLIC_KEY: [u8; 32] = [0; 32];

/// The first line of the signed message; the version of the message format.
pub const MESSAGE_HEADER: &str = "acs-release-v1";

/// Why an archive was not accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignatureError {
    /// The compiled-in key is all zeros.
    NoKey,
    /// The key (an override) is not 32 bytes of hex.
    BadKey,
    /// The `.sig` file is not one line of base64 holding 64 bytes.
    BadSignatureFile,
    /// The signature does not match the message and the key.
    Mismatch,
}

impl std::fmt::Display for SignatureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NoKey => "no release key in this build",
            Self::BadKey => "the release key is not valid",
            Self::BadSignatureFile => "the signature file is not valid",
            Self::Mismatch => "the signature does not match the download",
        })
    }
}

/// The message that is signed for `archive` of release `tag` whose SHA-256 (lowercase hex) is
/// `sha256_hex`.
#[must_use]
pub fn message(tag: &str, archive: &str, sha256_hex: &str) -> String {
    format!("{MESSAGE_HEADER}\n{tag}\n{archive}\n{sha256_hex}\n")
}

/// The lowercase hex SHA-256 of `bytes`.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// The key to verify with: `override_hex` (a debug build's test key) or the compiled-in one.
///
/// # Errors
///
/// [`SignatureError::NoKey`] for the all-zero key, [`SignatureError::BadKey`] for an override
/// that is not 64 hex digits.
pub fn release_key(override_hex: Option<&str>) -> Result<[u8; 32], SignatureError> {
    let key = match override_hex {
        Some(hex) => decode_hex32(hex).ok_or(SignatureError::BadKey)?,
        None => RELEASE_PUBLIC_KEY,
    };
    if key == [0; 32] {
        return Err(SignatureError::NoKey);
    }
    Ok(key)
}

/// Verifies that `signature_file` (base64, one line) signs the message of `tag`, `archive` and
/// the SHA-256 of `archive_bytes` under `key`.
///
/// # Errors
///
/// A [`SignatureError`]; nothing may be unpacked unless this is `Ok`.
pub fn verify(
    key: &[u8; 32],
    tag: &str,
    archive: &str,
    archive_bytes: &[u8],
    signature_file: &str,
) -> Result<(), SignatureError> {
    if *key == [0; 32] {
        return Err(SignatureError::NoKey);
    }
    let public = PublicKey::from_slice(key).map_err(|_| SignatureError::BadKey)?;
    let raw = b64_decode(signature_file.trim()).ok_or(SignatureError::BadSignatureFile)?;
    let signature = Signature::from_slice(&raw).map_err(|_| SignatureError::BadSignatureFile)?;
    let text = message(tag, archive, &sha256_hex(archive_bytes));
    public
        .verify(text.as_bytes(), &signature)
        .map_err(|_| SignatureError::Mismatch)
}

fn decode_hex32(hex: &str) -> Option<[u8; 32]> {
    let hex = hex.trim();
    if hex.len() != 64 || !hex.is_ascii() {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding.
#[must_use]
pub fn b64_encode(bytes: &[u8]) -> String {
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(B64[((n >> (18 - 6 * i)) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Decodes standard base64 with padding; `None` for anything else.
#[must_use]
pub fn b64_decode(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    if bytes.is_empty() || bytes.len() % 4 != 0 {
        return None;
    }
    let mut out = Vec::new();
    let groups = bytes.len() / 4;
    for (index, group) in bytes.chunks(4).enumerate() {
        let pad = group.iter().rev().take_while(|&&b| b == b'=').count();
        if pad > 2 || (pad > 0 && index + 1 != groups) {
            return None;
        }
        let mut n = 0u32;
        for &b in &group[..4 - pad] {
            let value = B64.iter().position(|&c| c == b)?;
            n = (n << 6) | u32::try_from(value).ok()?;
        }
        n <<= 6 * u32::try_from(pad).ok()?;
        let all = n.to_be_bytes();
        out.extend_from_slice(&all[1..4 - pad]);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_compact::{KeyPair, Seed};

    fn pair() -> KeyPair {
        KeyPair::from_seed(Seed::new([7u8; 32]))
    }

    fn signed(tag: &str, archive: &str, bytes: &[u8]) -> String {
        let text = message(tag, archive, &sha256_hex(bytes));
        b64_encode(pair().sk.sign(text.as_bytes(), None).as_ref())
    }

    fn key() -> [u8; 32] {
        *pair().pk
    }

    const NAME: &str = "acs-cli-x86_64-pc-windows-msvc.zip";

    #[test]
    fn the_message_is_four_lines_each_ended_by_a_newline() {
        assert_eq!(
            message("v1.0.0", "a.zip", "ab12"),
            "acs-release-v1\nv1.0.0\na.zip\nab12\n"
        );
    }

    /// The release workflow builds the message with the shell script; its `printf` format must
    /// produce exactly what [`message`] does, or no release would ever verify.
    #[test]
    fn the_signing_script_and_the_workflow_agree_with_the_message_builder() {
        let script = include_str!("../../../../.github/sign-release-message.sh");
        let line = script
            .lines()
            .find(|l| l.trim_start().starts_with("printf "))
            .expect("the script has a printf line");
        let format = line.split('\'').nth(1).expect("a single-quoted format");
        let args: Vec<&str> = line.split('\'').nth(2).unwrap().split_whitespace().collect();
        assert_eq!(args, ["\"$tag\"", "\"$name\"", "\"$sum\""]);
        let mut parts = format.split("%s");
        let mut built = parts.next().unwrap().to_owned();
        for (value, rest) in ["v1.2.3", "a.zip", "00ff"].iter().zip(parts) {
            built.push_str(value);
            built.push_str(rest);
        }
        let built = built.replace("\\n", "\n");
        assert_eq!(built, message("v1.2.3", "a.zip", "00ff"));
        assert!(script.contains("sha256sum"));

        let workflow = include_str!("../../../../.github/workflows/sign-release.yml");
        assert!(workflow.contains("sh .github/sign-release-message.sh \"$tag\" \"$archive\""));
        assert!(workflow.contains("openssl pkeyutl -sign -rawin"));
        assert!(workflow.contains("ACS_RELEASE_SIGNING_KEY"));
        assert!(workflow.contains("name: artifacts-signatures"));
    }

    #[test]
    fn sha256_matches_the_known_digest() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn a_good_signature_verifies() {
        let sig = signed("v1.0.0", NAME, b"archive");
        assert_eq!(verify(&key(), "v1.0.0", NAME, b"archive", &sig), Ok(()));
        assert_eq!(
            verify(&key(), "v1.0.0", NAME, b"archive", &format!("{sig}\r\n")),
            Ok(())
        );
    }

    #[test]
    fn a_wrong_key_a_tampered_file_or_another_tag_or_name_is_refused() {
        let sig = signed("v1.0.0", NAME, b"archive");
        let other = *KeyPair::from_seed(Seed::new([8u8; 32])).pk;
        assert_eq!(
            verify(&other, "v1.0.0", NAME, b"archive", &sig),
            Err(SignatureError::Mismatch)
        );
        assert_eq!(
            verify(&key(), "v1.0.0", NAME, b"archivX", &sig),
            Err(SignatureError::Mismatch)
        );
        assert_eq!(
            verify(&key(), "v1.0.1", NAME, b"archive", &sig),
            Err(SignatureError::Mismatch)
        );
        assert_eq!(
            verify(&key(), "v1.0.0", "other.zip", b"archive", &sig),
            Err(SignatureError::Mismatch)
        );
    }

    #[test]
    fn a_broken_signature_file_is_refused() {
        for bad in ["", "not base64!", "AAAA", "=AAA"] {
            assert_eq!(
                verify(&key(), "v1.0.0", NAME, b"a", bad),
                Err(SignatureError::BadSignatureFile),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn the_all_zero_key_refuses_everything() {
        let sig = signed("v1.0.0", NAME, b"archive");
        assert_eq!(
            verify(&[0; 32], "v1.0.0", NAME, b"archive", &sig),
            Err(SignatureError::NoKey)
        );
        assert_eq!(release_key(None), Err(SignatureError::NoKey));
        assert_eq!(
            SignatureError::NoKey.to_string(),
            "no release key in this build"
        );
    }

    #[test]
    fn a_key_override_is_64_hex_digits() {
        let hex: String = key().iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(release_key(Some(&hex)), Ok(key()));
        assert_eq!(release_key(Some("abcd")), Err(SignatureError::BadKey));
        assert_eq!(
            release_key(Some(&"zz".repeat(32))),
            Err(SignatureError::BadKey)
        );
    }

    #[test]
    fn base64_round_trips() {
        for len in 0..40usize {
            let bytes: Vec<u8> = (0..len)
                .map(|i| u8::try_from(i * 7 % 251).unwrap())
                .collect();
            let text = b64_encode(&bytes);
            if len == 0 {
                assert_eq!(b64_decode(&text), None);
            } else {
                assert_eq!(b64_decode(&text), Some(bytes), "{len}");
            }
        }
        assert_eq!(b64_encode(b"Man"), "TWFu");
        assert_eq!(b64_encode(b"Ma"), "TWE=");
        assert_eq!(b64_encode(b"M"), "TQ==");
    }
}
