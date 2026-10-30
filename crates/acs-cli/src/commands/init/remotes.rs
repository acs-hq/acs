//! The fetch refspec for ACS refs: `+refs/acs/*:refs/remotes/<remote>/acs/*`, added to every
//! configured remote so a plain `git fetch` brings the other machines' claims along.

use std::path::Path;
use std::process::{Command, Stdio};

use serde::Serialize;

/// What happened to one remote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteAction {
    /// The refspec was added to `remote.<name>.fetch`.
    Added,
    /// The refspec was already there.
    Present,
}

/// One remote and what `init` did with it.
#[derive(Debug, Clone, Serialize)]
pub struct RemoteReport {
    /// The remote's name.
    pub name: String,
    /// The refspec.
    pub refspec: String,
    /// What happened.
    pub action: RemoteAction,
}

/// The refspec for `remote`.
#[must_use]
pub fn refspec(remote: &str) -> String {
    format!("+refs/acs/*:refs/remotes/{remote}/acs/*")
}

fn git(work_tree: &Path) -> Command {
    let mut command = Command::new("git");
    command.current_dir(work_tree);
    for var in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_PREFIX",
    ] {
        command.env_remove(var);
    }
    command.stdin(Stdio::null());
    command
}

/// The names of the configured remotes (any remote with a `url`).
///
/// # Errors
///
/// A message when `git` cannot be run.
pub fn list(work_tree: &Path) -> Result<Vec<String>, String> {
    let output = git(work_tree)
        .args([
            "config",
            "--name-only",
            "--get-regexp",
            r"^remote\..*\.url$",
        ])
        .stderr(Stdio::null())
        .output()
        .map_err(|err| format!("cannot run git: {err}"))?;
    // Exit status 1 means "no match": no remotes.
    let mut names: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|key| {
            key.strip_prefix("remote.")
                .and_then(|rest| rest.strip_suffix(".url"))
                .map(str::to_owned)
        })
        .collect();
    names.sort();
    names.dedup();
    Ok(names)
}

/// Makes sure `remote.<name>.fetch` holds the ACS refspec; adds it with `git config --add` only
/// when it is not already there.
///
/// # Errors
///
/// A message when `git config` fails.
pub fn ensure(work_tree: &Path, name: &str) -> Result<RemoteReport, String> {
    let spec = refspec(name);
    let key = format!("remote.{name}.fetch");
    let existing = git(work_tree)
        .args(["config", "--get-all", &key])
        .stderr(Stdio::null())
        .output()
        .map_err(|err| format!("cannot run git: {err}"))?;
    let present = String::from_utf8_lossy(&existing.stdout)
        .lines()
        .any(|line| line.trim() == spec);
    let action = if present {
        RemoteAction::Present
    } else {
        let added = git(work_tree)
            .args(["config", "--add", &key, &spec])
            .output()
            .map_err(|err| format!("cannot run git: {err}"))?;
        if !added.status.success() {
            return Err(format!(
                "git config --add {key} failed: {}",
                String::from_utf8_lossy(&added.stderr).trim()
            ));
        }
        RemoteAction::Added
    };
    Ok(RemoteReport {
        name: name.to_owned(),
        refspec: spec,
        action,
    })
}
