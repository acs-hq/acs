//! The validated contents of the policy files, and the built-in defaults.

use acs_core::{Config, DEFAULT_SEALED, Destructive, Guards, Territory};
use serde::{Deserialize, Serialize};

/// Branch globs protected when `guards.toml` does not set `protected_branches`.
pub const DEFAULT_PROTECTED_BRANCHES: &[&str] = &["main", "release/*"];

/// Command patterns banned when `guards.toml` does not set `destructive.ban`. Ban patterns match
/// a whole command, so each ends in `*` to cover its arguments.
pub const DEFAULT_BAN: &[&str] = &["git push --force*", "git reset --hard*", "git clean -f*"];

/// The three policy files after parsing and defaulting: what the loader caches and compiles.
///
/// `guards` is *effective*: its `sealed` list starts with [`DEFAULT_SEALED`], and
/// `protected_branches` and `destructive.ban` hold the built-in defaults unless the file set
/// them. See [`resolve_guards`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyFiles {
    /// `config.toml` (or [`Config::default`] when absent).
    pub config: Config,
    /// `guards.toml` merged with the built-in defaults.
    pub guards: Guards,
    /// `territory.toml` (or an empty territory when absent).
    pub territory: Territory,
}

impl PolicyFiles {
    /// The policy in force when no policy file exists: default config, built-in default guards
    /// and no zones (the whole repository is `shared`).
    #[must_use]
    pub fn builtin() -> Self {
        Self {
            config: Config::default(),
            guards: builtin_guards(),
            territory: Territory::default(),
        }
    }
}

/// The built-in default guards: [`DEFAULT_SEALED`], [`DEFAULT_PROTECTED_BRANCHES`], the default
/// limits (`0` deletes outside claims, `50` per command) and [`DEFAULT_BAN`].
#[must_use]
pub fn builtin_guards() -> Guards {
    Guards {
        spec: None,
        sealed: strings(DEFAULT_SEALED),
        protected_branches: strings(DEFAULT_PROTECTED_BRANCHES),
        protect_outside_repo: false,
        destructive: Destructive {
            ban: strings(DEFAULT_BAN),
            ..Destructive::default()
        },
    }
}

/// Merges the contents of a `guards.toml` with the built-in defaults.
///
/// - `sealed` is always [`DEFAULT_SEALED`] followed by the file's entries (without duplicates):
///   the defaults cannot be removed.
/// - `protected_branches` and `destructive.ban` replace the defaults when the file sets them
///   (`has_protected_branches` / `has_ban`, even to an empty list) and keep the defaults when it
///   does not.
/// - The numeric limits come from the file; their serde defaults equal the built-in ones.
#[must_use]
pub fn resolve_guards(file: Guards, has_protected_branches: bool, has_ban: bool) -> Guards {
    let defaults = builtin_guards();
    Guards {
        spec: file.spec,
        sealed: effective_sealed(&file.sealed),
        protected_branches: if has_protected_branches {
            file.protected_branches
        } else {
            defaults.protected_branches
        },
        protect_outside_repo: file.protect_outside_repo,
        destructive: Destructive {
            ban: if has_ban {
                file.destructive.ban
            } else {
                defaults.destructive.ban
            },
            ..file.destructive
        },
    }
}

/// [`DEFAULT_SEALED`] followed by `extra`, without duplicates.
pub(crate) fn effective_sealed(extra: &[String]) -> Vec<String> {
    let mut sealed = strings(DEFAULT_SEALED);
    for glob in extra {
        if !sealed.contains(glob) {
            sealed.push(glob.clone());
        }
    }
    sealed
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|&s| s.to_owned()).collect()
}
