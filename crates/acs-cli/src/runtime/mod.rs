//! The shared runtime of `acs`: everything the hook, the CLI and the MCP server have in common.
//!
//! - [`RepoContext`] opens a repository: work tree, path resolver, compiled policy, the state
//!   database and the snapshot store, each failing independently (a broken state database must
//!   not stop the hook from enforcing sealed paths, SPEC section 11).
//! - [`seats`] resolves or creates the seat an event or a CLI call belongs to.
//! - [`paths`] normalizes the paths an event touches (file tools and shell commands) into the
//!   engine's targets.
//! - [`gate`] runs the decision engine against the current claims and acquires the claims.
//! - [`ops`] holds the operations later work packages call: [`status`], [`claim_paths`],
//!   [`release_paths`], [`set_intent`] and [`last_decision`].
//! - [`sync`] starts detached syncs and holds the lock that keeps one running at a time.
//! - [`protect`] supplies the overrides in force for a seat and the locations outside the
//!   repository that agents may never write.
//! - [`lore`] picks the lore a seat has not been shown yet for the paths of a tool call.

mod context;
pub mod gate;
pub mod lore;
mod ops;
pub mod paths;
pub mod protect;
pub mod records;
pub mod roots;
pub mod seats;
pub mod sync;

pub use context::{OpenError, RepoContext};
pub use ops::{
    COLLISION_WINDOW_HOURS, ClaimOutcome, ClaimView, CollisionView, MAX_INTENT_CHARS, OverrideItem,
    ReleaseReport, RemoteView, SeatSummary, StatusView, claim_paths, last_decision,
    record_collision, release_paths, set_intent, status,
};

/// Why a shared operation failed (as opposed to a claim the gate refused, which is a normal
/// outcome of [`claim_paths`]).
#[derive(Debug, thiserror::Error)]
pub enum OpError {
    /// The state database cannot be used; `acs doctor` repairs it.
    #[error("the state database is unavailable: {0}")]
    Unavailable(String),
    /// The state database failed during the operation.
    #[error(transparent)]
    Store(#[from] acs_store::StoreError),
    /// The input was not acceptable.
    #[error("{0}")]
    Invalid(String),
}
