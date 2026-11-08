//! Prefixed identifiers and the pilot-id rule.
//!
//! Identifiers are opaque strings of the form `<prefix><body>`, where the body is 1 to 64
//! characters from `[0-9a-z_-]`. This crate never generates identifiers (that needs randomness or a
//! clock); it only validates them when they are deserialized or parsed.

use std::fmt::{self, Write as _};
use std::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::ParseError;

/// Maximum length, in characters, of an identifier body (the part after the prefix).
const MAX_BODY_LEN: usize = 64;

fn validate_id(kind: &str, prefix: &str, value: &str) -> Result<(), ParseError> {
    let ok = value.strip_prefix(prefix).is_some_and(|body| {
        (1..=MAX_BODY_LEN).contains(&body.len())
            && body
                .bytes()
                .all(|b| b.is_ascii_digit() || b.is_ascii_lowercase() || b == b'_' || b == b'-')
    });
    if ok {
        Ok(())
    } else {
        Err(ParseError::new(format!(
            "invalid {kind} {value:?}: expected \"{prefix}\" followed by 1 to {MAX_BODY_LEN} \
             characters from [0-9a-z_-]"
        )))
    }
}

macro_rules! id_type {
    ($(#[$meta:meta])* $name:ident, $kind:literal, $prefix:literal) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
        #[serde(try_from = "String", into = "String")]
        #[schemars(extend("pattern" = concat!("^", $prefix, "[0-9a-z_-]{1,64}$")))]
        pub struct $name(String);

        impl $name {
            /// The fixed prefix every identifier of this type starts with.
            pub const PREFIX: &'static str = $prefix;

            /// The identifier as a string slice.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = ParseError;

            fn try_from(value: String) -> Result<Self, ParseError> {
                validate_id($kind, $prefix, &value)?;
                Ok(Self(value))
            }
        }

        impl FromStr for $name {
            type Err = ParseError;

            fn from_str(value: &str) -> Result<Self, ParseError> {
                Self::try_from(value.to_owned())
            }
        }

        impl From<$name> for String {
            fn from(id: $name) -> String {
                id.0
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

id_type! {
    /// Identifier of a seat: one agent session (or human editing session) working in a repository.
    ///
    /// Starts with `seat_`.
    SeatId, "seat id", "seat_"
}

id_type! {
    /// Identifier of a session record. This is the value carried by the `ACS-Session` commit trailer.
    ///
    /// Starts with `ses_`.
    SessionId, "session id", "ses_"
}

id_type! {
    /// Identifier of a claim.
    ///
    /// Starts with `clm_`.
    ClaimId, "claim id", "clm_"
}

id_type! {
    /// Identifier of a notice.
    ///
    /// Starts with `ntc_`.
    NoticeId, "notice id", "ntc_"
}

id_type! {
    /// Identifier of a lore entry, chosen when the entry is created and stable afterwards.
    ///
    /// Starts with `lore_`.
    LoreId, "lore id", "lore_"
}

id_type! {
    /// Identifier of a human override, which lets agents change files in `owned` zones they do
    /// not own.
    ///
    /// Starts with `ovr_`.
    OverrideId, "override id", "ovr_"
}

/// Derives the pilot id used in ref names from a git email address.
///
/// The pilot id is the first 16 lowercase hex characters of the SHA-256 of the email after
/// trimming surrounding whitespace and lowercasing. Ref names therefore never contain raw email
/// addresses. For example, `pilot_id("Sara@Example.com")` and `pilot_id(" sara@example.com ")`
/// are both `"259b65833bbadfd5"`.
#[must_use]
pub fn pilot_id(email: &str) -> String {
    let digest = Sha256::digest(email.trim().to_lowercase().as_bytes());
    digest
        .iter()
        .take(8)
        .fold(String::with_capacity(16), |mut out, byte| {
            // Writing to a String cannot fail.
            let _ = write!(out, "{byte:02x}");
            out
        })
}

/// Lowercase hex encoding of `bytes`.
pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_accept_valid_and_reject_invalid() {
        let cases: &[(&str, bool)] = &[
            ("seat_a", true),
            ("seat_0f3a-9_x", true),
            ("seat_", false),
            ("clm_a", false),
            ("seat_UPPER", false),
            ("seat_a b", false),
            ("Seat_a", false),
            ("", false),
        ];
        for &(input, ok) in cases {
            assert_eq!(input.parse::<SeatId>().is_ok(), ok, "{input:?}");
        }
        assert!(format!("seat_{}", "a".repeat(64)).parse::<SeatId>().is_ok());
        assert!(
            format!("seat_{}", "a".repeat(65))
                .parse::<SeatId>()
                .is_err()
        );
    }

    #[test]
    fn id_error_names_the_value_and_rule() {
        let err = "ses_ok".parse::<ClaimId>().unwrap_err().to_string();
        assert!(
            err.contains("claim id") && err.contains("\"ses_ok\"") && err.contains("\"clm_\""),
            "{err}"
        );
    }

    #[test]
    fn ids_serialize_as_plain_strings() {
        let id: LoreId = serde_json::from_str("\"lore_7f3a\"").unwrap();
        assert_eq!(serde_json::to_string(&id).unwrap(), "\"lore_7f3a\"");
        assert!(serde_json::from_str::<LoreId>("\"seat_7f3a\"").is_err());
    }

    #[test]
    fn pilot_id_test_vector() {
        // SHA-256("sara@example.com") = 259b65833bbadfd58ee66dde290489a6e51518339de4886d2331027751f0913a
        assert_eq!(pilot_id("sara@example.com"), "259b65833bbadfd5");
        assert_eq!(pilot_id("  Sara@Example.COM\n"), "259b65833bbadfd5");
        assert_eq!(pilot_id("sara@example.com").len(), 16);
    }
}
