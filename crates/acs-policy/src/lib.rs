//! ACS policy: TOML loading, validation, glob compilation and caching.
//!
//! The policy lives in three optional files under `.acs/policy/` (SPEC section 5):
//! `config.toml`, `guards.toml` and `territory.toml`. This crate
//!
//! - loads and validates them into the `acs-core` types, with errors that name the file, line
//!   and column, what is wrong and how to fix it ([`load`], [`PolicyError`]);
//! - applies the built-in default guards ([`builtin_guards`], [`resolve_guards`]);
//! - compiles them into a [`CompiledPolicy`] that implements the engine's
//!   [`PolicyView`](acs_core::gate::PolicyView): sealed globs, zone lookup where the most
//!   specific zone wins, protected branches, ban patterns and expanded team ownership;
//! - caches the last valid policy keyed by the hash of the files, and falls back to it (or to the
//!   built-in defaults) when the files are invalid ([`load_cached`], SPEC section 11).
//!
//! The crate performs no printing and does not depend on the state store: the caller supplies
//! the policy directory and the cache directory, and receives warnings as strings.

mod cache;
mod compiled;
mod error;
mod files;
pub mod glob;
mod load;

pub use cache::{CACHE_FILE, LoadOutcome, Origin, load_cached, policy_hash};
pub use compiled::CompiledPolicy;
pub use error::{PolicyError, PolicyFile, Problem, ProblemKind};
pub use files::{
    DEFAULT_BAN, DEFAULT_PROTECTED_BRANCHES, PolicyFiles, builtin_guards, resolve_guards,
};

use std::path::Path;

/// Options for loading a policy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LoadOptions {
    /// Match path and branch globs case-insensitively, as on case-insensitive filesystems.
    pub case_insensitive: bool,
}

/// A policy loaded strictly from disk.
#[derive(Debug)]
pub struct LoadedPolicy {
    /// The compiled policy.
    pub policy: CompiledPolicy,
    /// Non-fatal observations, for example a team without members.
    pub warnings: Vec<String>,
}

/// Loads, validates and compiles the policy in `policy_dir` (normally `.acs/policy`).
///
/// Absent files take their defaults: no `config.toml` is [`acs_core::Config::default`], no
/// `guards.toml` is [`builtin_guards`], no `territory.toml` is an empty territory (everything is
/// `shared`). An absent `policy_dir` is the same as three absent files. This function does not
/// use the cache and does not fall back: an invalid policy is an error. Hooks should call
/// [`load_cached`] instead.
///
/// # Errors
///
/// Returns [`PolicyError::Io`] if a file exists but cannot be read, and
/// [`PolicyError::Invalid`] listing every problem found otherwise.
pub fn load(policy_dir: &Path, opts: LoadOptions) -> Result<LoadedPolicy, PolicyError> {
    let policy = load::RawFiles::read(policy_dir)?.compile(opts.case_insensitive)?;
    let warnings = policy.warnings();
    Ok(LoadedPolicy { policy, warnings })
}
