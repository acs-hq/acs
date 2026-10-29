//! The decision the engine returns for an event, and the stable reason codes.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ids::ClaimId;

/// What the harness should do with the tool call.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Let the tool call proceed.
    Allow,
    /// Block the tool call. `message` tells the agent why and what to do instead.
    Deny,
    /// Ask the human to approve in the harness's own prompt.
    Ask,
    /// Reserved for deferred edits, which a later revision of the spec will define.
    /// Implementations MUST NOT emit it before then; consumers that do not support it MUST treat
    /// it as `deny`.
    Queue,
}

/// A stable machine-readable code saying why the engine decided as it did.
///
/// The set is closed: a new code is a spec change. The typical outcome of each code is listed in
/// the spec's reason-code table and by [`ReasonCode::typical_outcomes`].
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ReasonCode {
    /// Nothing is blocked. Outcome `allow`.
    Ok,
    /// `acs off` is active for this repository; the gate is disabled except for sealed paths.
    /// Outcome `allow`.
    GateOff,
    /// A target matches a sealed glob. Always denied for agents. Outcome `deny`.
    SealedPath,
    /// A shell command matches a `destructive.ban` pattern. Always denied. Outcome `deny`.
    BannedCommand,
    /// A history rewrite or force push targets a protected branch. Outcome `deny`.
    ProtectedBranch,
    /// A write, delete or move target resolves outside the repository root, including via a
    /// symlink or junction. Outcome `ask`.
    PathEscapesRepo,
    /// Deletes exceed `destructive.max_files_deleted_outside_claims`. Outcome `deny`.
    DeleteOutsideClaims,
    /// Deletes in one command exceed `destructive.max_files_deleted_per_command`. Outcome `ask`.
    DestructiveBudget,
    /// A destructive shell command whose targets cannot be determined. Outcome `ask`.
    DestructiveUnknownTargets,
    /// An overlapping live claim is held by another seat (claim rules, spec section 8).
    /// Outcome `deny`.
    ClaimConflict,
    /// A non-owner agent touched an `owned` zone. Implementations that do not yet enforce
    /// ownership treat `owned` zones as `shared` and never return this code. Outcome `deny`.
    OwnedTerritory,
    /// The state database is locked, corrupt or missing. Outcome `deny` for destructive actions,
    /// otherwise `allow` with a warning.
    StateUnavailable,
    /// The adapter could not parse the harness payload. Outcome `allow`.
    UnknownPayload,
    /// An edit was queued because of a claim conflict. Reserved together with the `queue`
    /// outcome; implementations MUST NOT return it yet. Outcome `queue`.
    ClaimQueued,
}

impl ReasonCode {
    /// Every reason code, in the order the spec lists them.
    pub const ALL: [Self; 14] = [
        Self::Ok,
        Self::GateOff,
        Self::SealedPath,
        Self::BannedCommand,
        Self::ProtectedBranch,
        Self::PathEscapesRepo,
        Self::DeleteOutsideClaims,
        Self::DestructiveBudget,
        Self::DestructiveUnknownTargets,
        Self::ClaimConflict,
        Self::OwnedTerritory,
        Self::StateUnavailable,
        Self::UnknownPayload,
        Self::ClaimQueued,
    ];

    /// The code as it appears on the wire.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::GateOff => "gate_off",
            Self::SealedPath => "sealed_path",
            Self::BannedCommand => "banned_command",
            Self::ProtectedBranch => "protected_branch",
            Self::PathEscapesRepo => "path_escapes_repo",
            Self::DeleteOutsideClaims => "delete_outside_claims",
            Self::DestructiveBudget => "destructive_budget",
            Self::DestructiveUnknownTargets => "destructive_unknown_targets",
            Self::ClaimConflict => "claim_conflict",
            Self::OwnedTerritory => "owned_territory",
            Self::StateUnavailable => "state_unavailable",
            Self::UnknownPayload => "unknown_payload",
            Self::ClaimQueued => "claim_queued",
        }
    }

    /// The outcomes an implementation produces with this code.
    #[must_use]
    pub const fn typical_outcomes(self) -> &'static [Outcome] {
        match self {
            Self::Ok | Self::GateOff | Self::UnknownPayload => &[Outcome::Allow],
            Self::SealedPath
            | Self::BannedCommand
            | Self::ProtectedBranch
            | Self::DeleteOutsideClaims
            | Self::ClaimConflict
            | Self::OwnedTerritory => &[Outcome::Deny],
            Self::PathEscapesRepo | Self::DestructiveBudget | Self::DestructiveUnknownTargets => {
                &[Outcome::Ask]
            }
            Self::StateUnavailable => &[Outcome::Deny, Outcome::Allow],
            Self::ClaimQueued => &[Outcome::Queue],
        }
    }
}

impl fmt::Display for ReasonCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What the engine returns for an event.
///
/// Unknown fields are ignored so that newer producers remain readable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Decision {
    /// What the harness should do.
    pub outcome: Outcome,
    /// Why, as a stable machine code.
    pub reason_code: ReasonCode,
    /// Guidance written for the agent: what is blocked, by whom, and what to do next. May be
    /// empty when nothing is blocked.
    #[serde(default)]
    pub message: String,
    /// Claims acquired while deciding this event.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub claims_acquired: Vec<ClaimId>,
    /// Lore and notices to inject into the agent's context.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context: Vec<String>,
    /// One-line warnings that do not change the outcome, such as a stale remote claims view or an
    /// invalid policy file that fell back to the last valid one. Adapters render them as context.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

impl Decision {
    /// A decision with the given outcome, reason and message, and nothing else set.
    #[must_use]
    pub fn new(outcome: Outcome, reason_code: ReasonCode, message: impl Into<String>) -> Self {
        Self {
            outcome,
            reason_code,
            message: message.into(),
            claims_acquired: Vec::new(),
            context: Vec::new(),
            warnings: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reason_codes_match_their_wire_names_and_are_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for code in ReasonCode::ALL {
            let wire = serde_json::to_value(code).unwrap();
            assert_eq!(wire, code.as_str());
            assert_eq!(serde_json::from_value::<ReasonCode>(wire).unwrap(), code);
            assert!(seen.insert(code), "{code} listed twice");
        }
        assert!(serde_json::from_str::<ReasonCode>("\"redirect\"").is_err());
    }

    #[test]
    fn all_lists_every_variant() {
        // The schema enumerates the variants as declared, so it catches a code missing from ALL.
        let schema = serde_json::to_value(schemars::schema_for!(ReasonCode)).unwrap();
        let variants: Vec<&str> = schema["oneOf"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["const"].as_str().unwrap())
            .collect();
        let all: Vec<&str> = ReasonCode::ALL.iter().map(|c| c.as_str()).collect();
        assert_eq!(variants, all);
    }

    #[test]
    fn decision_defaults_and_compact_output() {
        let decision: Decision =
            serde_json::from_str(r#"{"outcome":"allow","reason_code":"ok"}"#).unwrap();
        assert_eq!(decision, Decision::new(Outcome::Allow, ReasonCode::Ok, ""));
        assert_eq!(
            serde_json::to_string(&decision).unwrap(),
            r#"{"outcome":"allow","reason_code":"ok","message":""}"#
        );
    }
}
