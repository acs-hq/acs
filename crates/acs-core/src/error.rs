//! The error type for values that fail wire-level validation.

use std::fmt;

/// A wire value (an identifier, a path, a spec version, ...) failed validation.
///
/// The message is written to be shown to a person fixing a policy file or an adapter, so it names
/// the offending value and the rule it broke.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError(String);

impl ParseError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ParseError {}
