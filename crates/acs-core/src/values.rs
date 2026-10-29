//! Validated scalar values shared by several documents: the spec version, repo-relative paths,
//! symbol paths and content hashes.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::ParseError;
use crate::ids::hex;

/// The spec version this crate implements, as it appears in the `spec` field of documents.
pub const SPEC_VERSION: &str = "acs/0.1";

/// The `spec` field of a document: the version of the ACS spec it was written against.
///
/// The only supported value is `"acs/0.1"`. Readers MUST reject any other value.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "String", into = "String")]
#[schemars(extend("const" = "acs/0.1"))]
pub struct SpecVersion;

impl TryFrom<String> for SpecVersion {
    type Error = ParseError;

    fn try_from(value: String) -> Result<Self, ParseError> {
        if value == SPEC_VERSION {
            Ok(Self)
        } else {
            Err(ParseError::new(format!(
                "unsupported spec version {value:?}: this implementation supports only \"{SPEC_VERSION}\""
            )))
        }
    }
}

impl From<SpecVersion> for String {
    fn from(_: SpecVersion) -> String {
        SPEC_VERSION.to_owned()
    }
}

/// A repository-relative path in a stored record.
///
/// Uses forward slashes as separators, is relative to the repository root and is never empty.
/// Deserialization rejects empty strings, paths starting with `/` or `\`, and paths starting with
/// a Windows drive letter (`C:`). Producers MUST normalize before constructing one (no `.` or
/// `..` segments, symlinks resolved, case folded where the filesystem is case-insensitive);
/// that normalization is not checked here.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(try_from = "String", into = "String")]
#[schemars(extend("minLength" = 1, "pattern" = r"^(?:[^/\\A-Za-z]|[A-Za-z](?:[^:]|$))"))]
pub struct RepoPath(String);

impl RepoPath {
    /// The path as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for RepoPath {
    type Error = ParseError;

    fn try_from(value: String) -> Result<Self, ParseError> {
        let bytes = value.as_bytes();
        let reason = match bytes {
            [] => Some("it is empty"),
            [b'/' | b'\\', ..] => Some("it is absolute"),
            [drive, b':', ..] if drive.is_ascii_alphabetic() => {
                Some("it starts with a drive letter")
            }
            _ => None,
        };
        match reason {
            None => Ok(Self(value)),
            Some(reason) => Err(ParseError::new(format!(
                "invalid repository path {value:?}: {reason}; expected a non-empty path relative to the repository root"
            ))),
        }
    }
}

impl From<RepoPath> for String {
    fn from(path: RepoPath) -> String {
        path.0
    }
}

impl std::str::FromStr for RepoPath {
    type Err = ParseError;

    fn from_str(value: &str) -> Result<Self, ParseError> {
        Self::try_from(value.to_owned())
    }
}

impl AsRef<str> for RepoPath {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RepoPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The path of a symbol within its file, as segments joined by `.`.
///
/// For example `Refunds.apply` names the method `apply` of the class `Refunds`. Every segment is
/// non-empty. One symbol path *contains* another when its segments are a prefix of the other's
/// (a symbol contains itself).
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(try_from = "String", into = "String")]
#[schemars(extend("pattern" = r"^[^.]+(?:\.[^.]+)*$"))]
pub struct SymbolPath(String);

impl SymbolPath {
    /// The path as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether `self` is the same symbol as `other` or encloses it.
    #[must_use]
    pub fn contains(&self, other: &Self) -> bool {
        let mut inner = other.0.split('.');
        self.0
            .split('.')
            .all(|segment| inner.next() == Some(segment))
    }
}

impl TryFrom<String> for SymbolPath {
    type Error = ParseError;

    fn try_from(value: String) -> Result<Self, ParseError> {
        if value.split('.').any(str::is_empty) {
            return Err(ParseError::new(format!(
                "invalid symbol path {value:?}: expected non-empty segments joined by \".\""
            )));
        }
        Ok(Self(value))
    }
}

impl From<SymbolPath> for String {
    fn from(path: SymbolPath) -> String {
        path.0
    }
}

impl std::str::FromStr for SymbolPath {
    type Err = ParseError;

    fn from_str(value: &str) -> Result<Self, ParseError> {
        Self::try_from(value.to_owned())
    }
}

impl fmt::Display for SymbolPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A content hash: the lowercase hex SHA-256 digest (64 characters) of a file's bytes.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(try_from = "String", into = "String")]
#[schemars(extend("pattern" = "^[0-9a-f]{64}$"))]
pub struct ContentHash(String);

impl ContentHash {
    /// Hashes `content`.
    #[must_use]
    pub fn of(content: &[u8]) -> Self {
        Self(hex(&Sha256::digest(content)))
    }

    /// The digest as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ContentHash {
    type Error = ParseError;

    fn try_from(value: String) -> Result<Self, ParseError> {
        if value.len() == 64
            && value
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        {
            Ok(Self(value))
        } else {
            Err(ParseError::new(format!(
                "invalid content hash {value:?}: expected 64 lowercase hex characters (SHA-256)"
            )))
        }
    }
}

impl From<ContentHash> for String {
    fn from(hash: ContentHash) -> String {
        hash.0
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_version_accepts_only_the_supported_value() {
        assert!(serde_json::from_str::<SpecVersion>("\"acs/0.1\"").is_ok());
        for bad in ["\"acs/0.2\"", "\"acs/1.0\"", "\"\"", "1"] {
            assert!(serde_json::from_str::<SpecVersion>(bad).is_err(), "{bad}");
        }
        let err = serde_json::from_str::<SpecVersion>("\"acs/9\"")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("unsupported spec version") && err.contains("acs/0.1"),
            "{err}"
        );
        assert_eq!(serde_json::to_string(&SpecVersion).unwrap(), "\"acs/0.1\"");
    }

    #[test]
    fn repo_path_rules() {
        let cases: &[(&str, bool)] = &[
            ("src/payments/refund.ts", true),
            ("a", true),
            (".acs/policy/config.toml", true),
            ("ab:c", true),
            ("", false),
            ("/etc/passwd", false),
            ("\\\\server\\share", false),
            ("\\x", false),
            ("C:/x", false),
            ("c:x", false),
            ("C:", false),
        ];
        for &(input, ok) in cases {
            assert_eq!(input.parse::<RepoPath>().is_ok(), ok, "{input:?}");
        }
    }

    #[test]
    fn symbol_path_containment() {
        let sym = |s: &str| s.parse::<SymbolPath>().unwrap();
        let cases: &[(&str, &str, bool)] = &[
            ("Refunds", "Refunds", true),
            ("Refunds", "Refunds.apply", true),
            ("Refunds.apply", "Refunds", false),
            ("Refunds", "RefundsExtra", false),
            ("Refunds.apply", "Refunds.cancel", false),
        ];
        for &(outer, inner, expected) in cases {
            assert_eq!(
                sym(outer).contains(&sym(inner)),
                expected,
                "{outer} / {inner}"
            );
        }
        for bad in ["", ".", "a.", ".a", "a..b"] {
            assert!(bad.parse::<SymbolPath>().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn content_hash_of_known_input() {
        assert_eq!(
            ContentHash::of(b"").as_str(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert!(ContentHash::try_from("abc".to_owned()).is_err());
        assert!(
            ContentHash::try_from(
                "E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855".to_owned()
            )
            .is_err()
        );
    }
}
