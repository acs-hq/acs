//! ACS lore: short facts about parts of a repository, written by the people who own them and
//! shown to agents the first time they work there in a session (SPEC section 5.4).
//!
//! - [`format`] reads the lore files under `.acs/policy/lore/` into a [`LoreSet`], reporting
//!   broken files as [`LoreProblem`]s instead of failing.
//! - [`resolve`] finds the entries that apply to a set of paths, most specific first, and renders
//!   them for an agent's context within a character cap.
//! - [`stale`] measures how much of an entry's scope changed since the revision it was written
//!   against, and caches the answer per `HEAD`.
//! - [`propose`] holds agents' proposals until an owner reviews them. Agents never write lore
//!   directly: `.acs/**` is sealed.
//!
//! The crate performs no printing and knows nothing about seats or the state database: callers
//! pass in the work tree, the paths and the directories to use.

pub mod format;
pub mod propose;
pub mod resolve;
pub mod stale;

pub use format::{LORE_DIR, LoreEntry, LoreProblem, LoreSet};
pub use propose::{Proposal, ProposalStore, Proposer};
pub use resolve::{Match, Served, render, resolve};
pub use stale::{StaleCache, Staleness};
