//! Git operations for lore (Phase 6): measuring how much of an entry's scope changed since the
//! revision it was written against, and committing an accepted entry on a branch of its own
//! without touching the work tree or the user's index.

use std::path::{Path, PathBuf};

use crate::GitError;
use crate::ledger::signing_configured;
use crate::refs::{SIGN_LIMIT, checked, git_output, git_text, resolve_ref, run_limited};
use crate::repo::git_command;

/// How many lines a scope had at a revision, and how many changed since.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScopeChange {
    /// Lines in the scope's text files at the earlier revision.
    pub lines_then: u64,
    /// Lines added plus lines deleted in the scope's text files between the two revisions.
    pub lines_changed: u64,
    /// Files in the scope at the earlier revision.
    pub files_then: u64,
}

/// The full id of the commit `rev` names, or `None` when it names none in this repository (an
/// unknown or abbreviated-and-ambiguous revision, or a shallow clone missing it).
///
/// # Errors
///
/// [`GitError::Command`] when `git` cannot be started.
pub fn resolve_commit(work_tree: &Path, rev: &str) -> Result<Option<String>, GitError> {
    if rev.is_empty() || rev.starts_with('-') {
        return Ok(None);
    }
    resolve_ref(work_tree, &format!("{rev}^{{commit}}"))
}

/// Measures how much of a scope changed between the commits `since` and `until`.
///
/// `pathspecs` narrow what git reads (`:(glob)` pathspecs or directories; empty means the whole
/// tree) and `keep` decides exactly which paths belong to the scope, so the caller's own glob
/// dialect is the one that counts. Binary files are ignored; renames count as a delete plus an
/// add.
///
/// # Errors
///
/// [`GitError::Command`] when a `git` command fails, for example because a revision is unknown
/// (check it with [`resolve_commit`] first).
pub fn scope_change(
    work_tree: &Path,
    since: &str,
    until: &str,
    pathspecs: &[String],
    keep: &dyn Fn(&str) -> bool,
) -> Result<ScopeChange, GitError> {
    let empty_tree = {
        let args = ["hash-object", "-t", "tree", "--stdin"];
        checked(&args, git_output(work_tree, &args, Some(b""), &[])?)?
    };
    let then = numstat(work_tree, &empty_tree, since, pathspecs, keep)?;
    let changed = numstat(work_tree, since, until, pathspecs, keep)?;
    Ok(ScopeChange {
        lines_then: then.iter().map(|(added, _)| added).sum(),
        lines_changed: changed.iter().map(|(added, deleted)| added + deleted).sum(),
        files_then: u64::try_from(then.len()).unwrap_or(u64::MAX),
    })
}

/// `git diff --numstat` between two trees or commits, as (added, deleted) per kept text file.
fn numstat(
    work_tree: &Path,
    from: &str,
    to: &str,
    pathspecs: &[String],
    keep: &dyn Fn(&str) -> bool,
) -> Result<Vec<(u64, u64)>, GitError> {
    let mut args: Vec<&str> = vec![
        "-c",
        "core.quotePath=false",
        "diff",
        "--numstat",
        "-z",
        "--no-renames",
        "--no-ext-diff",
        "--no-textconv",
        from,
        to,
        "--",
    ];
    args.extend(pathspecs.iter().map(String::as_str));
    let output = git_output(work_tree, &args, None, &[])?;
    if !output.status.success() {
        checked(&args, output)?;
        return Ok(Vec::new());
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut rows = Vec::new();
    // With `-z` and no renames each record is `added \t deleted \t path \0`.
    for record in text.split('\0').filter(|r| !r.is_empty()) {
        let mut fields = record.splitn(3, '\t');
        let (Some(added), Some(deleted), Some(path)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let (Ok(added), Ok(deleted)) = (added.parse::<u64>(), deleted.parse::<u64>()) else {
            continue; // `-` for binary files
        };
        if keep(path) {
            rows.push((added, deleted));
        }
    }
    Ok(rows)
}

/// A commit made by [`commit_file_on_branch`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchCommit {
    /// The new commit.
    pub commit: String,
    /// The commit it was made on top of.
    pub parent: String,
    /// Whether the branch already existed (the commit then goes on top of it).
    pub branch_existed: bool,
    /// Whether the commit is signed.
    pub signed: bool,
}

/// Commits one file on the branch `branch` (a short name such as `acs/lore-7f3a`), leaving the
/// work tree, the index and `HEAD` alone.
///
/// The commit goes on top of the branch when it exists, otherwise on top of `base` (usually
/// `HEAD`), and the branch is created or moved to it. The user's own identity and signing
/// configuration are used, as for any commit they make; if signing is configured but fails,
/// the commit is made unsigned and [`BranchCommit::signed`] says so.
///
/// # Errors
///
/// [`GitError::Command`] when `base` names no commit, when the user has no git identity, or when
/// another process moved the branch at the same moment.
pub fn commit_file_on_branch(
    work_tree: &Path,
    branch: &str,
    base: &str,
    path: &str,
    content: &[u8],
    message: &str,
) -> Result<BranchCommit, GitError> {
    let full = format!("refs/heads/{branch}");
    let existing = resolve_ref(work_tree, &full)?;
    let parent = match &existing {
        Some(tip) => tip.clone(),
        None => resolve_commit(work_tree, base)?.ok_or_else(|| GitError::Command {
            args: "rev-parse".to_owned(),
            detail: format!("`{base}` names no commit; make a first commit before this"),
        })?,
    };

    let index = TempIndex::new(work_tree)?;
    let env = [("GIT_INDEX_FILE", index.path_str())];
    let args = ["read-tree", parent.as_str()];
    checked(&args, git_output(work_tree, &args, None, &env)?)?;
    let args = ["hash-object", "-w", "--stdin"];
    let blob = checked(&args, git_output(work_tree, &args, Some(content), &[])?)?;
    let cacheinfo = format!("100644,{blob},{path}");
    let args = ["update-index", "--add", "--cacheinfo", cacheinfo.as_str()];
    checked(&args, git_output(work_tree, &args, None, &env)?)?;
    let args = ["write-tree"];
    let tree = checked(&args, git_output(work_tree, &args, None, &env)?)?;
    drop(index);

    let make = |sign: bool| {
        let mut command = git_command(work_tree);
        command.arg("commit-tree").arg(&tree).arg("-p").arg(&parent);
        if sign {
            command.arg("-S");
        }
        command.arg("-m").arg(message);
        run_limited(command, SIGN_LIMIT, "commit-tree")
    };
    let sign = signing_configured(work_tree)?;
    let (commit, signed) = match sign.then(|| make(true)) {
        Some(Ok(id)) => (id, true),
        _ => (make(false)?, false),
    };
    let args = [
        "update-ref",
        full.as_str(),
        commit.as_str(),
        existing.as_deref().unwrap_or(""),
    ];
    checked(&args, git_output(work_tree, &args, None, &[])?)?;
    Ok(BranchCommit {
        commit,
        parent,
        branch_existed: existing.is_some(),
        signed,
    })
}

/// The id of the commit `HEAD` points to, or `None` before the first commit.
///
/// # Errors
///
/// [`GitError::Command`] when `git` cannot be started.
pub fn head_commit(work_tree: &Path) -> Result<Option<String>, GitError> {
    resolve_commit(work_tree, "HEAD")
}

/// The shortest unambiguous form of `commit` of at least 12 characters.
///
/// # Errors
///
/// [`GitError::Command`] when `commit` is unknown.
pub fn short_commit(work_tree: &Path, commit: &str) -> Result<String, GitError> {
    git_text(work_tree, &["rev-parse", "--short=12", commit])
}

/// A scratch index file inside the git directory, removed when dropped.
struct TempIndex {
    path: PathBuf,
    text: String,
}

impl TempIndex {
    fn new(work_tree: &Path) -> Result<Self, GitError> {
        let git_dir = git_text(work_tree, &["rev-parse", "--absolute-git-dir"])?;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let path =
            PathBuf::from(git_dir).join(format!("acs-lore-index.{}.{nanos}", std::process::id()));
        let text = path.to_string_lossy().into_owned();
        Ok(Self { path, text })
    }

    fn path_str(&self) -> &str {
        &self.text
    }
}

impl Drop for TempIndex {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        git(d, &["init", "-q", "-b", "main"]);
        git(d, &["config", "user.email", "omar@example.com"]);
        git(d, &["config", "user.name", "Omar"]);
        git(d, &["config", "commit.gpgsign", "false"]);
        dir
    }

    fn write(dir: &Path, rel: &str, text: &str) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn scope_change_counts_lines_in_scope_only() {
        let dir = repo();
        let d = dir.path();
        write(d, "pay/a.rs", "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n");
        write(d, "other/b.rs", "x\ny\n");
        git(d, &["add", "."]);
        git(d, &["commit", "-q", "-m", "one"]);
        let since = head_commit(d).unwrap().unwrap();
        write(d, "pay/a.rs", "1\n2\n3\nfour\n5\n6\n7\n8\n9\n10\n");
        write(d, "other/b.rs", "changed\n");
        git(d, &["commit", "-q", "-am", "two"]);
        let keep = |p: &str| p.starts_with("pay/");
        let change = scope_change(d, &since, "HEAD", &[":(glob)pay/**".to_owned()], &keep).unwrap();
        assert_eq!(
            change,
            ScopeChange {
                lines_then: 10,
                lines_changed: 2,
                files_then: 1
            }
        );
        // The filter is what decides, whatever the pathspecs let through.
        let change = scope_change(d, &since, "HEAD", &[], &keep).unwrap();
        assert_eq!(change.lines_changed, 2);
    }

    #[test]
    fn unknown_revisions_resolve_to_none() {
        let dir = repo();
        let d = dir.path();
        assert_eq!(head_commit(d).unwrap(), None);
        write(d, "a", "a\n");
        git(d, &["add", "."]);
        git(d, &["commit", "-q", "-m", "one"]);
        assert!(resolve_commit(d, "a1b2c3d4e5").unwrap().is_none());
        assert!(resolve_commit(d, "--all").unwrap().is_none());
        assert!(resolve_commit(d, "HEAD").unwrap().is_some());
    }

    #[test]
    fn commit_on_branch_leaves_work_tree_index_and_head_alone() {
        let dir = repo();
        let d = dir.path();
        write(d, "src/a.rs", "a\n");
        git(d, &["add", "."]);
        git(d, &["commit", "-q", "-m", "one"]);
        write(d, "src/a.rs", "edited, not staged\n");
        write(d, "staged.txt", "staged\n");
        git(d, &["add", "staged.txt"]);
        let head = head_commit(d).unwrap().unwrap();

        let made = commit_file_on_branch(
            d,
            "acs/lore-7f3a",
            "HEAD",
            ".acs/policy/lore/src/7f3a.md",
            b"+++\nbody\n",
            "Add lore lore_7f3a",
        )
        .unwrap();
        assert_eq!(made.parent, head);
        assert!(!made.branch_existed);
        assert_eq!(head_commit(d).unwrap().unwrap(), head);
        assert_eq!(
            git(d, &["status", "--porcelain"]),
            "M src/a.rs\nA  staged.txt"
        );
        assert_eq!(
            git(d, &["show", "acs/lore-7f3a:.acs/policy/lore/src/7f3a.md"]),
            "+++\nbody"
        );
        assert_eq!(git(d, &["show", "acs/lore-7f3a:src/a.rs"]), "a");
        assert_eq!(
            git(d, &["log", "-1", "--format=%ae %s", "acs/lore-7f3a"]),
            "omar@example.com Add lore lore_7f3a"
        );

        // A second commit goes on top of the branch.
        let again = commit_file_on_branch(
            d,
            "acs/lore-7f3a",
            "HEAD",
            ".acs/policy/lore/src/7f3a.md",
            b"+++\nbody 2\n",
            "Edit lore lore_7f3a",
        )
        .unwrap();
        assert!(again.branch_existed);
        assert_eq!(again.parent, made.commit);
    }
}
