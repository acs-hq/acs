//! The seam between the MCP protocol layer and ACS itself.
//!
//! [`Backend`] has one method per tool, with typed, serializable requests and replies. The MCP
//! server (this crate) turns tool calls into these requests and the replies into text for the
//! agent; `acs-cli` implements the trait on top of the shared runtime operations, and the tests
//! implement it with a fake. Nothing here knows about MCP, SQLite or git.
//!
//! Implementations are called from the server's single protocol thread, one call at a time, and
//! should open whatever they need per call (opening a repository is cheap).

use serde::{Deserialize, Serialize};

/// Who is calling: what the protocol layer could learn about the harness session behind a tool
/// call. The backend binds it to a seat (decision 0011 section 5).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Caller {
    /// The harness name (`claude-code`, `antigravity`, ...), from `--harness` or inferred from
    /// what the harness sent. `None` when it could not be told.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    /// The harness's own session id, when the harness exposed one to this server. It can be
    /// stale (Claude Code after `--continue` or `/clear`), so a backend must check that it
    /// names a live seat of `harness` and otherwise fall back to the most recently seen live
    /// seat of that harness in the server's work tree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// The harness's id of this very tool call (Claude Code's `claudecode/toolUseId`), equal to
    /// the `tool_use_id` of the matching pre-tool hook event. Not used for binding yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
}

/// Why a backend call failed. Each variant becomes a tool result with guidance for the agent,
/// never a protocol error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BackendError {
    /// No ACS session could be bound to this caller yet. Carries the text shown to the agent.
    #[error("{0}")]
    NoSession(String),
    /// The request was not acceptable (for example an empty intent). Carries the explanation.
    #[error("{0}")]
    Invalid(String),
    /// ACS cannot do this right now (state database, repository). Carries the explanation.
    #[error("{0}")]
    Unavailable(String),
}

/// The seat a call is bound to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeatInfo {
    /// The seat id.
    pub seat_id: String,
    /// The `ses_` id of its session (the `ACS-Session` trailer value).
    pub session_id: String,
    /// The harness.
    pub harness: String,
    /// The pilot (the person responsible for the seat).
    pub pilot: String,
    /// What the agent said it is doing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<String>,
}

/// A live claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimInfo {
    /// The claimed file (repository-relative).
    pub path: String,
    /// The symbol inside the file, for symbol claims.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol_path: Option<String>,
    /// The pilot holding it.
    pub holder: String,
    /// The holder's harness, when known (`human` for claims made from the command line).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    /// `local` or `remote` (another machine's, as last fetched).
    pub source: String,
    /// When it expires unless refreshed (RFC 3339).
    pub expires_at: String,
}

/// The reply to [`Backend::status`].
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct StatusReply {
    /// The repository's work tree.
    pub worktree: String,
    /// The checked-out branch; absent when detached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// The branch work is integrated into.
    pub target_branch: String,
    /// `acs off` is active: claims are not enforced (sealed paths still are).
    pub gate_off: bool,
    /// Claims are switched off in this repository: nothing is claimed and nothing needs to be.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub claims_off: bool,
    /// The other parts of ACS switched off here that an agent would notice (`lore`, `guards`,
    /// ...), by their `[features]` key.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub also_off: Vec<String>,
    /// The caller's seat; absent when no session is bound yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seat: Option<SeatInfo>,
    /// The caller's live claims.
    pub my_claims: Vec<ClaimInfo>,
    /// Everyone else's live claims.
    pub other_claims: Vec<ClaimInfo>,
    /// Path globs no agent may write.
    pub sealed: Vec<String>,
    /// Things the agent should know: no session yet, stale remote view, policy fallbacks,
    /// an unavailable state database.
    pub notes: Vec<String>,
}

/// The request of [`Backend::claim`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimRequest {
    /// Files or directories, absolute or relative to the repository root.
    pub paths: Vec<String>,
}

/// The reply to [`Backend::claim`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ClaimReply {
    /// The caller holds these files now.
    Granted {
        /// The files claimed (repository-relative).
        paths: Vec<String>,
        /// Warnings that did not change the outcome.
        warnings: Vec<String>,
        /// Lore for these files that the caller had not been shown yet in this session.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        lore: Vec<String>,
    },
    /// Nothing was claimed.
    Refused {
        /// The machine-readable reason (the spec's reason code).
        reason_code: String,
        /// Guidance in the gate's wording: names the holder or the rule and what to do next.
        message: String,
    },
}

/// The request of [`Backend::release`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseRequest {
    /// Files or directories, absolute or relative to the repository root.
    pub paths: Vec<String>,
}

/// The reply to [`Backend::release`].
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ReleaseReply {
    /// Files whose claims were released.
    pub released: Vec<String>,
    /// Files the caller held no claim on.
    pub not_held: Vec<String>,
    /// Paths outside the repository.
    pub outside: Vec<String>,
}

/// The request of [`Backend::intent`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntentRequest {
    /// What the agent is about to do.
    pub text: String,
}

/// The reply to [`Backend::intent`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntentReply {
    /// The intent as stored (trimmed, cut to the length limit).
    pub intent: String,
}

/// The last decision ACS made for a seat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionInfo {
    /// `allow`, `deny` or `ask`.
    pub outcome: String,
    /// The machine-readable reason.
    pub reason_code: String,
    /// What the agent was told (empty when nothing was blocked).
    pub message: String,
    /// When it was decided (RFC 3339).
    pub at: String,
}

/// The reply to [`Backend::why`].
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct WhyReply {
    /// The seat's last decision; absent when it has had none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<DecisionInfo>,
}

/// The request of [`Backend::lore_propose`].
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LoreProposeRequest {
    /// Path globs the entry applies to. May be empty when `replaces` is set.
    pub scope: Vec<String>,
    /// The entry's text.
    pub text: String,
    /// The id of an existing entry this corrects.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaces: Option<String>,
    /// Why the agent proposes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

/// The reply to [`Backend::lore_propose`].
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LoreProposeReply {
    /// The proposal's id (`prop_...`).
    pub proposal_id: String,
    /// The scope as stored.
    pub scope: Vec<String>,
    /// The entry it would replace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaces: Option<String>,
    /// Who is expected to review it (the entry's or the zone's owner), when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewer: Option<String>,
}

/// Which switchable parts of ACS the repository has on, as far as the tools are concerned.
///
/// The tool list and the server's instructions follow it (decision 0034): a tool whose feature
/// is off is not listed, so nothing tells an agent to claim where claims are off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Available {
    /// Claims are on: `acs_claim` and `acs_release` are listed.
    pub claims: bool,
    /// Lore is on: `acs_lore_propose` is listed.
    pub lore: bool,
}

impl Default for Available {
    /// Everything on, which is also the answer when the repository cannot be read.
    fn default() -> Self {
        Self {
            claims: true,
            lore: true,
        }
    }
}

/// What the six tools need from ACS.
///
/// A [`BackendError::NoSession`] from any method means the caller has no seat yet; `status`
/// instead answers with `seat: None` and a note, since the repository view is still useful.
pub trait Backend: Send + Sync + 'static {
    /// Which features are on, read when the harness asks for the instructions and the tool
    /// list. Never fails: a repository that cannot be read counts as everything on, and the
    /// tools then explain the problem when called.
    fn available(&self) -> Available {
        Available::default()
    }

    /// `acs_status`: claims, sealed paths and notes, from the caller's point of view.
    ///
    /// # Errors
    ///
    /// [`BackendError::Unavailable`] when the repository cannot be opened.
    fn status(&self, caller: &Caller) -> Result<StatusReply, BackendError>;

    /// `acs_claim`: claim files for the caller's seat. A claim the gate refuses is
    /// [`ClaimReply::Refused`], not an error.
    ///
    /// # Errors
    ///
    /// [`BackendError::NoSession`] without a seat; [`BackendError::Unavailable`].
    fn claim(&self, caller: &Caller, request: &ClaimRequest) -> Result<ClaimReply, BackendError>;

    /// `acs_release`: release the caller's own claims.
    ///
    /// # Errors
    ///
    /// [`BackendError::NoSession`] without a seat; [`BackendError::Unavailable`].
    fn release(
        &self,
        caller: &Caller,
        request: &ReleaseRequest,
    ) -> Result<ReleaseReply, BackendError>;

    /// `acs_intent`: record what the caller is about to do.
    ///
    /// # Errors
    ///
    /// [`BackendError::NoSession`] without a seat; [`BackendError::Invalid`] for an empty
    /// text; [`BackendError::Unavailable`].
    fn intent(&self, caller: &Caller, request: &IntentRequest)
    -> Result<IntentReply, BackendError>;

    /// `acs_why`: the caller's last decision.
    ///
    /// # Errors
    ///
    /// [`BackendError::NoSession`] without a seat; [`BackendError::Unavailable`].
    fn why(&self, caller: &Caller) -> Result<WhyReply, BackendError>;

    /// `acs_lore_propose`: hold a lore proposal for the owner's review. Nothing is written to
    /// the repository.
    ///
    /// # Errors
    ///
    /// [`BackendError::NoSession`] without a seat; [`BackendError::Invalid`] for a proposal that
    /// does not pass the checks (the message says what to fix); [`BackendError::Unavailable`].
    fn lore_propose(
        &self,
        caller: &Caller,
        request: &LoreProposeRequest,
    ) -> Result<LoreProposeReply, BackendError>;
}
