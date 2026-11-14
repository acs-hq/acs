//! The repository's main branch, for `default_target_branch` and `protected_branches` (issue 67,
//! decision 0040).
//!
//! The built-in defaults assume `main`. `acs init` looks for the branch the repository really
//! integrates into and, when it is not `main`, writes it into the policy files it creates:
//!
//! 1. the remote's `HEAD` (`refs/remotes/<remote>/HEAD`, set by `git clone` and by
//!    `git remote set-head`), `origin` first, then the other remotes in order;
//! 2. else the current branch, when it is `main`, `master` or `develop`.
//!
//! Anything else is not a guess worth writing: the defaults stay and `acs init` says what it
//! could not tell.

use std::path::Path;
use std::process::{Command, Stdio};

use serde::Serialize;

/// Current-branch names taken as the main branch when no remote says otherwise.
pub const LIKELY_MAIN_BRANCHES: [&str; 3] = ["main", "master", "develop"];

/// Where the branch name came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BranchSource {
    /// `refs/remotes/<remote>/HEAD`.
    RemoteHead {
        /// The remote's name.
        remote: String,
    },
    /// The branch checked out now.
    CurrentBranch,
}

impl BranchSource {
    /// For people: `origin/HEAD` or `the current branch`.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::RemoteHead { remote } => format!("{remote}/HEAD"),
            Self::CurrentBranch => "the current branch".to_owned(),
        }
    }
}

/// The repository's main branch and how it was found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DefaultBranch {
    /// The branch name, without `refs/heads/`.
    pub name: String,
    /// Where it came from.
    pub source: BranchSource,
}

fn git(work_tree: &Path) -> Command {
    let mut command = Command::new("git");
    command.arg("-C").arg(work_tree);
    for var in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_PREFIX",
    ] {
        command.env_remove(var);
    }
    command.stdin(Stdio::null()).stderr(Stdio::null());
    command
}

/// The target of the symbolic ref `name`, if it is one.
fn symbolic_ref(work_tree: &Path, name: &str) -> Option<String> {
    let out = git(work_tree)
        .args(["symbolic-ref", "--quiet", name])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let target = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (!target.is_empty()).then_some(target)
}

/// The branch the remote's `HEAD` points at.
fn remote_head(work_tree: &Path, remote: &str) -> Option<String> {
    let prefix = format!("refs/remotes/{remote}/");
    symbolic_ref(work_tree, &format!("{prefix}HEAD"))?
        .strip_prefix(&prefix)
        .filter(|name| !name.is_empty() && *name != "HEAD")
        .map(str::to_owned)
}

/// The branch checked out now (also when it has no commits yet).
#[must_use]
pub fn current_branch(work_tree: &Path) -> Option<String> {
    symbolic_ref(work_tree, "HEAD")?
        .strip_prefix("refs/heads/")
        .map(str::to_owned)
}

/// Finds the repository's main branch; `None` when neither rule gives one.
#[must_use]
pub fn detect(work_tree: &Path) -> Option<DefaultBranch> {
    let mut remotes = acs_git::repo::remotes(work_tree).unwrap_or_default();
    if let Some(at) = remotes.iter().position(|r| r == "origin") {
        let origin = remotes.remove(at);
        remotes.insert(0, origin);
    }
    for remote in remotes {
        if let Some(name) = remote_head(work_tree, &remote) {
            return Some(DefaultBranch {
                name,
                source: BranchSource::RemoteHead { remote },
            });
        }
    }
    current_branch(work_tree)
        .filter(|name| LIKELY_MAIN_BRANCHES.contains(&name.as_str()))
        .map(|name| DefaultBranch {
            name,
            source: BranchSource::CurrentBranch,
        })
}

/// The branches that exist here: local branches and remote-tracking ones (without the remote's
/// name), plus the current branch even before its first commit.
#[must_use]
pub fn known_branches(work_tree: &Path) -> Option<Vec<String>> {
    let out = git(work_tree)
        .args([
            "for-each-ref",
            "--format=%(refname)",
            "refs/heads",
            "refs/remotes",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let remotes = acs_git::repo::remotes(work_tree).unwrap_or_default();
    let mut names: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            if let Some(name) = line.strip_prefix("refs/heads/") {
                return Some(name.to_owned());
            }
            let rest = line.strip_prefix("refs/remotes/")?;
            // Remote names may contain `/`, so strip a known remote rather than the first part.
            remotes
                .iter()
                .find_map(|r| rest.strip_prefix(r.as_str())?.strip_prefix('/'))
                .filter(|name| *name != "HEAD")
                .map(str::to_owned)
        })
        .collect();
    names.extend(current_branch(work_tree));
    names.sort();
    names.dedup();
    Some(names)
}
