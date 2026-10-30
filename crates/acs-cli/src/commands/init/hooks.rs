//! The git hook step of `acs init` and `acs doctor`: the two entry points below are the only
//! places they touch hooks, and both call WP2.4's installer (`acs_git::hooks`, decision 0015).

use acs_git::RepoLayout;
use acs_git::hooks::{self, Action, HooksError, LocationKind};
use serde::Serialize;

/// One hook file `init` handled.
#[derive(Debug, Clone, Serialize)]
pub struct HookLine {
    /// The hook name (`pre-commit`, ...).
    pub name: String,
    /// What happened: `created`, `injected`, `wrapped`, `updated` or `unchanged`.
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
        /// Something the person should know (husky 9 hook files are committed).
        #[serde(skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    /// Nothing was installed, on purpose (hooks are disabled in the git configuration).
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
    /// Something is missing or outdated; the text says what, per hook.
    Unhealthy(String),
    /// The state could not be determined; the text says why.
    Unknown(String),
}

fn action_word(action: Action) -> &'static str {
    match action {
        Action::Created => "created",
        Action::Injected => "injected",
        Action::Wrapped => "wrapped",
        Action::Updated => "updated",
        Action::Unchanged => "unchanged",
    }
}

/// Installs the ACS git hooks, chained with any existing ones. Idempotent.
#[must_use]
pub fn install(layout: &RepoLayout) -> HooksInstall {
    match hooks::install(layout) {
        Ok(report) => {
            let note = (report.location.kind == LocationKind::HuskyV9).then(|| {
                "husky 9 detected: ACS added a marked block to the committed .husky/<hook> files. \
                 Teammates without `acs` on PATH get a one-line warning per hook; commit the \
                 change to share it."
                    .to_owned()
            });
            HooksInstall::Done {
                hooks: report
                    .hooks
                    .iter()
                    .map(|h| HookLine {
                        name: h.name.to_owned(),
                        action: action_word(h.action).to_owned(),
                    })
                    .collect(),
                note,
            }
        }
        Err(HooksError::Disabled(value)) => HooksInstall::Pending {
            note: format!(
                "git hooks are disabled (core.hooksPath = {value}), so none were installed"
            ),
        },
        Err(err) => HooksInstall::Failed {
            error: err.to_string(),
        },
    }
}

/// Inspects the installed git hooks without changing anything.
#[must_use]
pub fn health(layout: &RepoLayout) -> HooksHealth {
    let status = match hooks::status(layout) {
        Ok(status) => status,
        Err(HooksError::Disabled(value)) => {
            return HooksHealth::Unknown(format!(
                "git hooks are disabled (core.hooksPath = {value})"
            ));
        }
        Err(err) => return HooksHealth::Unknown(err.to_string()),
    };
    if status.healthy() {
        return HooksHealth::Healthy;
    }
    let mut problems = Vec::new();
    for hook in &status.hooks {
        let problem = if !hook.installed {
            "the ACS block is missing"
        } else if !hook.up_to_date {
            "the ACS block is outdated"
        } else if !hook.active {
            "the file is not executable, so git skips it"
        } else {
            continue;
        };
        problems.push(format!("{}: {problem}", hook.name));
    }
    let mut text = problems.join("; ");
    if status.framework.is_some() {
        text.push_str(
            ". husky and `pre-commit install` regenerate hook files and drop the ACS block.",
        );
    }
    HooksHealth::Unhealthy(text)
}
