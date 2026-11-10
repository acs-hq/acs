//! Stored records: seats, claims, session records, notices and the published claims document.

use std::collections::BTreeMap;

use jiff::{SignedDuration, Timestamp};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

use crate::decision::{Outcome, ReasonCode};
use crate::ids::{ClaimId, LoreId, NoticeId, OverrideId, SeatId, SessionId};
use crate::values::{ContentHash, RepoPath, SpecVersion, SymbolPath};

/// One agent session (or human editing session) working in a repository.
///
/// There is exactly one seat per harness session. A seat is created on the first event of a
/// session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Seat {
    /// Identifier of the seat.
    pub seat_id: SeatId,
    /// The pilot (the human responsible for the seat), identified by git email address.
    pub pilot: String,
    /// Name of the harness, for example `claude-code`.
    pub harness: String,
    /// The harness's own session identifier.
    pub harness_session_id: String,
    /// Absolute path of the worktree the seat works in. In a session record, only the name of
    /// the worktree's folder: records are pushed, and absolute paths carry user names.
    pub worktree: String,
    /// The checked-out branch. Absent when HEAD is detached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// The branch this work will be integrated into. Claims only conflict within one target
    /// branch.
    pub target_branch: String,
    /// When the seat was created (UTC).
    pub started_at: Timestamp,
    /// When the seat last produced an event (UTC).
    pub last_seen_at: Timestamp,
}

/// What a claim covers: a whole file, or one symbol inside a file.
///
/// On the wire this is `{"file": "<path>"}` or `{"symbol": "<path>", "symbol_path": "<a.b.c>"}`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, JsonSchema)]
#[serde(untagged, deny_unknown_fields)]
pub enum ClaimScope {
    /// A whole file.
    File {
        /// Repository-relative path of the file.
        file: RepoPath,
    },
    /// One symbol (function, class, method, ...) within a file.
    Symbol {
        /// Repository-relative path of the file that contains the symbol.
        symbol: RepoPath,
        /// The symbol's path inside the file, segments joined by `.`.
        symbol_path: SymbolPath,
    },
}

/// Flat wire form of [`ClaimScope`], so a malformed scope gets one clear error message.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClaimScopeWire {
    file: Option<RepoPath>,
    symbol: Option<RepoPath>,
    symbol_path: Option<SymbolPath>,
}

impl<'de> Deserialize<'de> for ClaimScope {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match ClaimScopeWire::deserialize(deserializer)? {
            ClaimScopeWire {
                file: Some(file),
                symbol: None,
                symbol_path: None,
            } => Ok(Self::File { file }),
            ClaimScopeWire {
                file: None,
                symbol: Some(symbol),
                symbol_path: Some(symbol_path),
            } => Ok(Self::Symbol {
                symbol,
                symbol_path,
            }),
            _ => Err(serde::de::Error::custom(
                "invalid claim scope: expected {\"file\"} or {\"symbol\", \"symbol_path\"}",
            )),
        }
    }
}

impl ClaimScope {
    /// The file the scope lives in.
    #[must_use]
    pub fn path(&self) -> &RepoPath {
        match self {
            Self::File { file } | Self::Symbol { symbol: file, .. } => file,
        }
    }

    /// Whether two scopes overlap (claim rule 1): the same file; for two symbols in one file, the
    /// same symbol or one containing the other; and a file claim overlaps every symbol claim in
    /// that file.
    #[must_use]
    pub fn overlaps(&self, other: &Self) -> bool {
        if self.path() != other.path() {
            return false;
        }
        match (self, other) {
            (Self::Symbol { symbol_path: a, .. }, Self::Symbol { symbol_path: b, .. }) => {
                a.contains(b) || b.contains(a)
            }
            _ => true,
        }
    }
}

/// Who created a claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClaimOrigin {
    /// Acquired by an agent through the gate.
    Agent,
    /// Taken by a human, for example by the editor extension or `acs watch`.
    Human,
}

/// A live claim: a seat's exclusive right to change a scope until it is released or expires.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Claim {
    /// Identifier of the claim.
    pub claim_id: ClaimId,
    /// The seat holding the claim.
    pub seat_id: SeatId,
    /// The holder's pilot (git email).
    pub pilot: String,
    /// What the claim covers.
    pub scope: ClaimScope,
    /// The branch the holder integrates into. Claims only conflict within one target branch.
    pub target_branch: String,
    /// When the claim was acquired (UTC).
    pub acquired_at: Timestamp,
    /// When the holder last proved it is alive (UTC). Expiry counts from here.
    pub heartbeat_at: Timestamp,
    /// Seconds after `heartbeat_at` at which the claim expires.
    pub ttl_secs: u64,
    /// Who created the claim.
    pub origin: ClaimOrigin,
}

impl Claim {
    /// Whether the claim has expired at `now`: `now - heartbeat_at >= ttl_secs`.
    #[must_use]
    pub fn is_expired(&self, now: Timestamp) -> bool {
        let ttl = SignedDuration::from_secs(i64::try_from(self.ttl_secs).unwrap_or(i64::MAX));
        now.duration_since(self.heartbeat_at) >= ttl
    }

    /// Whether this claim comes before `other` when two machines hold the same scope: the earlier
    /// `acquired_at`, then the lower `claim_id`. It looks at nothing but the two claims, so every
    /// machine that compares them gets the same answer.
    #[must_use]
    pub fn precedes(&self, other: &Self) -> bool {
        (self.acquired_at, self.claim_id.as_str()) < (other.acquired_at, other.claim_id.as_str())
    }
}

/// Why a claim ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseReason {
    /// The holder released it explicitly.
    Released,
    /// It ran past its TTL without a heartbeat.
    Expired,
    /// It was released because the session ended.
    SessionEnd,
}

/// One claim in a session's history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClaimHistoryEntry {
    /// Identifier of the claim.
    pub claim_id: ClaimId,
    /// What the claim covered.
    pub scope: ClaimScope,
    /// When it was acquired (UTC).
    pub acquired_at: Timestamp,
    /// When it ended (UTC). Absent if it had not ended when the record was assembled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub released_at: Option<Timestamp>,
    /// Why it ended. Absent if it had not ended when the record was assembled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_reason: Option<ReleaseReason>,
}

/// A file a session changed, with content hashes before and after.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileChange {
    /// Repository-relative path of the file.
    pub path: RepoPath,
    /// Hash of the content before the session's first change. `null` if the file did not exist.
    #[serde(default)]
    pub pre_hash: Option<ContentHash>,
    /// Hash of the content after the session's last change. `null` if the file was deleted.
    #[serde(default)]
    pub post_hash: Option<ContentHash>,
}

/// A guard decision worth listing individually in the record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NotableDecision {
    /// When the decision was made (UTC).
    pub at: Timestamp,
    /// Why the engine decided as it did.
    pub reason_code: ReasonCode,
    /// What the engine decided.
    pub outcome: Outcome,
    /// The repository-relative path or, for shell commands, the command text that triggered the
    /// decision. Recorders MUST NOT store secrets here (records sync to the git remote) and
    /// SHOULD truncate long commands.
    pub path_or_command: String,
}

/// Summary of the gate's decisions during a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GuardDecisions {
    /// How many times each reason code was returned. Codes never returned are omitted.
    #[schemars(extend("propertyNames" = {"$ref": "#/$defs/ReasonCode"}))]
    pub counts: BTreeMap<ReasonCode, u64>,
    /// The denials and prompts worth reading individually, oldest first.
    pub notable: Vec<NotableDecision>,
}

/// How long a human override lasts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OverrideMode {
    /// One tool call: the first call that needs the override uses it up.
    Once,
    /// The rest of the session of the first seat that uses it.
    Session,
}

/// A human override a session used: the pilot let the session's agent change files in `owned`
/// zones it does not own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OverrideUse {
    /// Identifier of the override.
    pub override_id: OverrideId,
    /// The glob the override covers, repository-relative.
    pub pattern: String,
    /// How long it lasts.
    pub mode: OverrideMode,
    /// The human who granted it, identified by git email address.
    pub granted_by: String,
    /// When it was granted (UTC).
    pub granted_at: Timestamp,
    /// The files the session changed or claimed under it, each once, in order of first use.
    pub paths: Vec<RepoPath>,
}

/// Why a session record was finalized.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EndedReason {
    /// The harness reported the end of the session.
    Clean,
    /// No end event arrived; the record was assembled later and `ended_at` is the last time the
    /// seat was seen.
    Crashed,
}

/// The ledger entry for one session: who worked, what was claimed, what changed, what was denied.
///
/// Records are synced to the git remote, so they hold only structural data (paths, hashes,
/// counts), never prompts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionRecord {
    /// The spec version this record conforms to. Always `acs/0.1`.
    pub spec: SpecVersion,
    /// Identifier of the record; the value of the `ACS-Session` commit trailer.
    pub session_id: SessionId,
    /// The seat the session ran on.
    pub seat: Seat,
    /// What the agent said it was doing, from the `acs_intent` tool. Absent if never declared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<String>,
    /// Every claim the seat held, in acquisition order.
    pub claims: Vec<ClaimHistoryEntry>,
    /// Every file the session changed.
    pub files_changed: Vec<FileChange>,
    /// Summary of the gate's decisions.
    pub guard_decisions: GuardDecisions,
    /// Full hashes of the commits that carry this session's `ACS-Session` trailer.
    pub commits: Vec<String>,
    /// Lore entries injected into the session.
    pub lore_loaded: Vec<LoreId>,
    /// Human overrides the session used. Omitted when there were none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overrides: Vec<OverrideUse>,
    /// When the session started (UTC).
    pub started_at: Timestamp,
    /// When the session ended (UTC). For crashed sessions, the last time the seat was seen.
    pub ended_at: Timestamp,
    /// Whether the session ended cleanly.
    pub ended_reason: EndedReason,
    /// Version of the `acs` binary that wrote the record.
    pub acs_version: String,
}

/// What a notice tells the agent about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NoticeKind {
    /// An exported signature changed in code the target seat's claims reference.
    InterfaceChanged,
    /// A claim the target seat was waiting for was released.
    ClaimReleased,
    /// A deferred edit of the target seat was applied.
    QueuedEditApplied,
    /// A lore entry in scope of the target seat may be out of date.
    LoreStale,
}

/// A one-way system message from ACS to a seat, delivered in the seat's next hook context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Notice {
    /// Identifier of the notice.
    pub notice_id: NoticeId,
    /// What the notice is about.
    pub kind: NoticeKind,
    /// The seat the notice is for.
    pub target_seat: SeatId,
    /// The text shown to the agent. Written for the agent: what changed and what to do about it.
    pub body: String,
    /// When the notice was created (UTC).
    pub created_at: Timestamp,
    /// When it was delivered (UTC). Absent until delivered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivered_at: Option<Timestamp>,
}

/// The `claims.json` file published in `refs/acs/claims/<pilot-id>`: one pilot's live claims
/// across all their seats.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClaimsDocument {
    /// The spec version this document conforms to. Always `acs/0.1`.
    pub spec: SpecVersion,
    /// The pilot (git email) who owns the ref.
    pub pilot: String,
    /// When the document was last rewritten (UTC).
    pub updated_at: Timestamp,
    /// The pilot's live claims.
    pub claims: Vec<Claim>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str) -> ClaimScope {
        ClaimScope::File {
            file: path.parse().unwrap(),
        }
    }

    fn symbol(path: &str, symbol_path: &str) -> ClaimScope {
        ClaimScope::Symbol {
            symbol: path.parse().unwrap(),
            symbol_path: symbol_path.parse().unwrap(),
        }
    }

    #[test]
    fn scope_overlap_table() {
        let cases = [
            (file("a.rs"), file("a.rs"), true),
            (file("a.rs"), file("b.rs"), false),
            (file("a.rs"), symbol("a.rs", "f"), true),
            (symbol("a.rs", "f"), file("a.rs"), true),
            (file("a.rs"), symbol("b.rs", "f"), false),
            (symbol("a.rs", "f"), symbol("a.rs", "f"), true),
            (symbol("a.rs", "f"), symbol("a.rs", "g"), false),
            (symbol("a.rs", "C"), symbol("a.rs", "C.m"), true),
            (symbol("a.rs", "C.m"), symbol("a.rs", "C"), true),
            (symbol("a.rs", "C.m"), symbol("a.rs", "C.n"), false),
            (symbol("a.rs", "C"), symbol("a.rs", "CD"), false),
            (symbol("a.rs", "f"), symbol("b.rs", "f"), false),
        ];
        for (a, b, expected) in cases {
            assert_eq!(a.overlaps(&b), expected, "{a:?} / {b:?}");
            assert_eq!(b.overlaps(&a), expected, "symmetry: {b:?} / {a:?}");
        }
    }

    #[test]
    fn scope_wire_shapes() {
        let f: ClaimScope = serde_json::from_str(r#"{"file":"a/b.rs"}"#).unwrap();
        assert_eq!(f, file("a/b.rs"));
        assert_eq!(serde_json::to_string(&f).unwrap(), r#"{"file":"a/b.rs"}"#);
        let s: ClaimScope =
            serde_json::from_str(r#"{"symbol":"a/b.rs","symbol_path":"C.m"}"#).unwrap();
        assert_eq!(s, symbol("a/b.rs", "C.m"));
        assert_eq!(
            serde_json::to_string(&s).unwrap(),
            r#"{"symbol":"a/b.rs","symbol_path":"C.m"}"#
        );
        for bad in [
            r#"{}"#,
            r#"{"symbol":"a.rs"}"#,
            r#"{"symbol_path":"f"}"#,
            r#"{"file":"a.rs","symbol":"a.rs","symbol_path":"f"}"#,
            r#"{"file":"a.rs","extra":1}"#,
            r#"{"file":"/abs.rs"}"#,
        ] {
            assert!(serde_json::from_str::<ClaimScope>(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn claim_expiry_boundary() {
        let at = |secs: i64| Timestamp::from_second(1_800_000_000 + secs).unwrap();
        let claim = Claim {
            claim_id: "clm_1".parse().unwrap(),
            seat_id: "seat_1".parse().unwrap(),
            pilot: "sara@example.com".into(),
            scope: file("a.rs"),
            target_branch: "main".into(),
            acquired_at: at(0),
            heartbeat_at: at(100),
            ttl_secs: 900,
            origin: ClaimOrigin::Agent,
        };
        let cases = [
            (0, false),
            (100, false),
            (999, false),
            (1000, true),
            (5000, true),
        ];
        for (now, expected) in cases {
            assert_eq!(claim.is_expired(at(now)), expected, "now = +{now}s");
        }
        let forever = Claim {
            ttl_secs: u64::MAX,
            ..claim
        };
        assert!(!forever.is_expired(at(10_000_000)));
    }
}
