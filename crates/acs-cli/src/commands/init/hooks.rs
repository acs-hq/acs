//! The git hook step of `acs init` and `acs doctor`.
//!
//! **STUB.** The installer is WP2.4's `acs_git::hooks` (`install`, `status`), which is not
//! merged into this branch yet. Both entry points below are the only places `init` and `doctor`
//! touch hooks; replacing their bodies with calls into `acs_git::hooks` completes the step.

use acs_git::RepoLayout;
use serde::Serialize;

/// One hook file `init` handled.
#[derive(Debug, Clone, Serialize)]
pub struct HookLine {
    /// The hook name (`pre-commit`, ...).
    pub name: String,
    /// What happened (`created`, `unchanged`, ...).
    pub action: String,
}

/// The result of installing the git hooks.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum HooksInstall {
    /// The installer ran.
    Done {
        /// One entry per hook.
        hooks: Vec<HookLine>,
    },
    /// The installer is not available in this build.
    Pending {
        /// Why, for the person.
        note: String,
    },
    /// The installer failed.
    Failed {
        /// What went wrong.
        error: String,
    },
}

/// The health of the installed git hooks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HooksHealth {
    /// All hooks are installed, current and active.
    Healthy,
    /// Something is missing or outdated; the text says what.
    Unhealthy(String),
    /// The check is not available in this build.
    Unknown(String),
}

/// Installs the ACS git hooks, chained with any existing ones. Idempotent.
// TODO(WP2.4): call `acs_git::hooks::install(layout)` and map its `InstallReport`.
#[must_use]
pub fn install(_layout: &RepoLayout) -> HooksInstall {
    HooksInstall::Pending {
        note: "git hooks are not installed: the installer is not part of this build yet".to_owned(),
    }
}

/// Inspects the installed git hooks without changing anything.
// TODO(WP2.4): call `acs_git::hooks::status(layout)` and map `HooksStatus::healthy`.
#[must_use]
pub fn health(_layout: &RepoLayout) -> HooksHealth {
    HooksHealth::Unknown("git hook status is not available in this build yet".to_owned())
}
