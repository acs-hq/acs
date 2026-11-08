//! ACS core: pure types and the decision engine; performs no I/O.
//!
//! This crate defines the contracts of the Agent Control System (spec `acs/0.1`): the canonical
//! [`ToolEvent`] adapters produce, the [`Decision`] the engine returns, the stored records, and the
//! policy-file types. The JSON Schemas under `spec/schemas/` are generated from these types (see
//! [`schema`]). Nothing here touches the filesystem, network, processes or the clock: time is
//! always passed in.

mod decision;
pub mod engine;
mod error;
mod event;
pub mod gate;
mod ids;
mod policy_types;
mod records;
pub mod schema;
mod values;

pub use decision::{Decision, Outcome, ReasonCode};
pub use error::ParseError;
pub use event::{EditInfo, Phase, Tool, ToolEvent, ToolKind, ToolResult};
pub use ids::{ClaimId, LoreId, NoticeId, OverrideId, SeatId, SessionId, pilot_id};
pub use jiff::Timestamp;
pub use policy_types::{
    Config, DEFAULT_SEALED, Destructive, Guards, LedgerConfig, LoreFrontMatter, Owner, Territory,
    Zone, ZoneKind, split_front_matter,
};
pub use records::{
    Claim, ClaimHistoryEntry, ClaimOrigin, ClaimScope, ClaimsDocument, EndedReason, FileChange,
    GuardDecisions, NotableDecision, Notice, NoticeKind, OverrideMode, OverrideUse, ReleaseReason,
    Seat, SessionRecord,
};
pub use values::{ContentHash, RepoPath, SPEC_VERSION, SpecVersion, SymbolPath};
