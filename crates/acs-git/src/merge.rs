//! Three-way merges of file contents and revert commits for `acs undo` (WP4.3).
//!
//! - [`merge_file`] runs `git merge-file -p` on three byte strings.
//! - [`session_commit_paths`] finds the files the commits that carry a session's `ACS-Session`
//!   trailer touched.
//! - [`head_matches`] tells whether a path in the work tree equals its content in `HEAD`.
//! - [`commit_only`] commits exactly the given paths with a new commit (history is never
//!   rewritten).

use std::collections::BTreeSet;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use acs_core::ContentHash;

use crate::GitError;
use crate::repo::{BlobSource, git_command, index_blobs};
use crate::trailers::{DEFAULT_COMMENT_CHAR, session_ids};

/// The result of a three-way merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeOutcome {
    /// The merge is clean: this is the merged content.
    Clean(Vec<u8>),
    /// The changes overlap (or the content is binary): nothing usable was produced.
    Conflict,
}

/// A scratch file that is removed when it goes out of scope.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn scratch_file(dir: &Path, label: &str, bytes: &[u8]) -> Result<Scratch, GitError> {
    fs::create_dir_all(dir).map_err(|e| GitError::io("creating", dir, e))?;
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = dir.join(format!("merge-{}-{n}-{label}", std::process::id()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| GitError::io("creating", &path, e))?;
    let scratch = Scratch(path);
    file.write_all(bytes)
        .map_err(|e| GitError::io("writing", &scratch.0, e))?;
    Ok(scratch)
}

/// Merges with `git merge-file -p <current> <base> <other>`: the result is `current` with the
/// change from `base` to `other` applied. `scratch` is a directory for the three temporary files.
///
/// # Errors
///
/// [`GitError`] when the scratch files cannot be written or `git` cannot be run. Overlapping
/// changes are not an error: they give [`MergeOutcome::Conflict`].
pub fn merge_file(
    scratch: &Path,
    current: &[u8],
    base: &[u8],
    other: &[u8],
) -> Result<MergeOutcome, GitError> {
    let current_file = scratch_file(scratch, "current", current)?;
    let base_file = scratch_file(scratch, "base", base)?;
    let other_file = scratch_file(scratch, "other", other)?;
    let output = Command::new("git")
        .args(["merge-file", "-p"])
        .arg(&current_file.0)
        .arg(&base_file.0)
        .arg(&other_file.0)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| GitError::Command {
            args: "merge-file".to_owned(),
            detail: e.to_string(),
        })?;
    match output.status.code() {
        Some(0) => Ok(MergeOutcome::Clean(output.stdout)),
        // The exit status is the number of conflicts, up to 127; larger values are errors, and
        // git refuses to merge binary files that way too, which is a conflict for our purposes.
        _ => Ok(MergeOutcome::Conflict),
    }
}

/// The repository paths touched by the commits reachable from `HEAD` whose message carries
/// `ACS-Session: <session_id>`. Empty when there are no such commits or the repository has no
/// commits yet.
///
/// # Errors
///
/// [`GitError::Command`] when `git` cannot be run.
pub fn session_commit_paths(
    work_tree: &Path,
    session_id: &str,
) -> Result<BTreeSet<String>, GitError> {
    let fail = |detail: String| GitError::Command {
        args: "log".to_owned(),
        detail,
    };
    let log = git_command(work_tree)
        .args(["log", "--format=%H%x1f%B%x1e", "--grep"])
        .arg(format!(
            "^{}: {session_id}$",
            crate::trailers::SESSION_TOKEN
        ))
        .arg("-E")
        .arg("HEAD")
        .stderr(Stdio::null())
        .output()
        .map_err(|e| fail(e.to_string()))?;
    let mut paths = BTreeSet::new();
    if !log.status.success() {
        return Ok(paths); // no HEAD yet
    }
    let text = String::from_utf8_lossy(&log.stdout).into_owned();
    for record in text.split('\u{1e}') {
        let Some((hash, message)) = record.trim().split_once('\u{1f}') else {
            continue;
        };
        if !session_ids(message, DEFAULT_COMMENT_CHAR)
            .iter()
            .any(|id| id == session_id)
        {
            continue;
        }
        let changed = git_command(work_tree)
            .args([
                "diff-tree",
                "--root",
                "-r",
                "-m",
                "--no-commit-id",
                "--name-only",
                "-z",
            ])
            .arg(hash)
            .stderr(Stdio::null())
            .output()
            .map_err(|e| fail(e.to_string()))?;
        paths.extend(
            String::from_utf8_lossy(&changed.stdout)
                .split('\0')
                .filter(|p| !p.is_empty())
                .map(str::to_owned),
        );
    }
    Ok(paths)
}

/// Whether the work-tree file `path` (given as the content hash it has now, `None` when it does
/// not exist) is exactly what `HEAD` holds for it (`None`: `HEAD` has no such file).
///
/// # Errors
///
/// [`GitError::Command`] when `git` cannot be run.
pub fn head_matches(
    work_tree: &Path,
    path: &str,
    current: Option<&ContentHash>,
) -> Result<bool, GitError> {
    let blobs = index_blobs(work_tree, &[path.to_owned()], BlobSource::Head)?;
    let head = blobs.into_iter().next().flatten();
    Ok(match (head, current) {
        (None, None) => true,
        (Some(bytes), Some(hash)) => ContentHash::of(&bytes) == *hash,
        _ => false,
    })
}

/// What [`commit_only`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitOutcome {
    /// A commit was made; this is its abbreviated id.
    Committed(String),
    /// The paths already match `HEAD`: there was nothing to commit.
    NothingToCommit,
    /// `git commit` failed (a hook rejected it, the identity is unknown, ...): its output.
    Failed(String),
}

/// Commits the work-tree state of `paths` with `message`, and only those paths (`git commit
/// --only`): other staged changes stay staged and are not part of the commit. Deleted paths are
/// committed as deletions. User hooks run; if one rejects the commit the work tree is left as it
/// is and [`CommitOutcome::Failed`] says why.
///
/// # Errors
///
/// [`GitError::Command`] when `git` cannot be run.
pub fn commit_only(
    work_tree: &Path,
    paths: &[String],
    message: &str,
) -> Result<CommitOutcome, GitError> {
    if paths.is_empty() {
        return Ok(CommitOutcome::NothingToCommit);
    }
    let fail = |detail: String| GitError::Command {
        args: "commit".to_owned(),
        detail,
    };
    // Stage the paths first: `--only` takes deletions of tracked files, but a restored file that
    // is new to the index (the session had deleted it) needs the add.
    let add = git_command(work_tree)
        .arg("--literal-pathspecs")
        .args(["add", "-A", "--"])
        .args(paths)
        .output()
        .map_err(|e| fail(e.to_string()))?;
    if !add.status.success() {
        let text = output_text(&add);
        // A path that is neither in the work tree nor known to git has nothing to commit.
        if text.contains("did not match any files") {
            return Ok(CommitOutcome::NothingToCommit);
        }
        return Ok(CommitOutcome::Failed(text));
    }
    let output = git_command(work_tree)
        .arg("--literal-pathspecs")
        .args(["commit", "--only", "-q", "-m", message, "--"])
        .args(paths)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| fail(e.to_string()))?;
    if output.status.success() {
        let head = git_command(work_tree)
            .args(["rev-parse", "--short", "HEAD"])
            .output()
            .map_err(|e| fail(e.to_string()))?;
        return Ok(CommitOutcome::Committed(
            String::from_utf8_lossy(&head.stdout).trim().to_owned(),
        ));
    }
    let text = output_text(&output);
    if text.contains("nothing to commit") || text.contains("no changes added") {
        return Ok(CommitOutcome::NothingToCommit);
    }
    Ok(CommitOutcome::Failed(text))
}

fn output_text(output: &std::process::Output) -> String {
    let mut text = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    let out = String::from_utf8_lossy(&output.stdout);
    if !out.trim().is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(out.trim());
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) {
        let status = git_command(dir)
            .args(["-c", "user.email=t@example.com", "-c", "user.name=t"])
            .args(args)
            .stdout(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    #[test]
    fn merging_applies_the_reverse_change_and_reports_overlaps() {
        let dir = tempfile::tempdir().unwrap();
        let base = b"1\n2\n3\n4\n5\n6\n7\n8\n";
        let other = b"1\n2\n3\n4\n5\n6\n7\n8\nadded by a\n";
        // `current` has an unrelated edit at the top.
        let current = b"top\n1\n2\n3\n4\n5\n6\n7\n8\nadded by a\n";
        // Reverse: current with other -> base applied (here: base is "post", other is "pre").
        let merged = merge_file(dir.path(), current, other, base).unwrap();
        assert_eq!(
            merged,
            MergeOutcome::Clean(b"top\n1\n2\n3\n4\n5\n6\n7\n8\n".to_vec())
        );
        let clash = merge_file(dir.path(), b"1\nx\n3\n", b"1\n2\n3\n", b"1\ny\n3\n").unwrap();
        assert_eq!(clash, MergeOutcome::Conflict);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn session_commits_are_found_by_their_trailer() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        git(repo, &["init", "-q", "-b", "main", "."]);
        fs::write(repo.join("a.txt"), "a\n").unwrap();
        git(repo, &["add", "-A"]);
        git(repo, &["commit", "-q", "-m", "init"]);
        assert!(session_commit_paths(repo, "ses_x").unwrap().is_empty());
        fs::write(repo.join("a.txt"), "b\n").unwrap();
        fs::write(repo.join("n.txt"), "n\n").unwrap();
        git(repo, &["add", "-A"]);
        git(repo, &["commit", "-q", "-m", "work\n\nACS-Session: ses_x"]);
        fs::write(repo.join("o.txt"), "o\n").unwrap();
        git(repo, &["add", "-A"]);
        git(repo, &["commit", "-q", "-m", "other\n\nACS-Session: ses_y"]);
        let paths = session_commit_paths(repo, "ses_x").unwrap();
        assert_eq!(
            paths.into_iter().collect::<Vec<_>>(),
            vec!["a.txt".to_owned(), "n.txt".to_owned()]
        );
    }

    #[test]
    fn commit_only_commits_the_named_paths_and_deletions() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        git(repo, &["init", "-q", "-b", "main", "."]);
        fs::write(repo.join("a.txt"), "a\n").unwrap();
        fs::write(repo.join("b.txt"), "b\n").unwrap();
        git(repo, &["add", "-A"]);
        git(repo, &["commit", "-q", "-m", "init"]);
        fs::remove_file(repo.join("a.txt")).unwrap();
        fs::write(repo.join("b.txt"), "changed\n").unwrap();
        fs::write(repo.join("c.txt"), "c\n").unwrap();
        git(repo, &["add", "c.txt"]);
        let outcome = commit_only(repo, &["a.txt".to_owned()], "Undo\n\nUndo test").unwrap();
        assert!(
            matches!(outcome, CommitOutcome::Committed(_)),
            "{outcome:?}"
        );
        let shown = git_command(repo)
            .args(["show", "--name-status", "--format=", "HEAD"])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&shown.stdout).trim(), "D\ta.txt");
        // Nothing left to commit for a path that matches HEAD.
        let again = commit_only(repo, &["a.txt".to_owned()], "Undo").unwrap();
        assert_eq!(again, CommitOutcome::NothingToCommit);
        assert!(head_matches(repo, "a.txt", None).unwrap());
        assert!(!head_matches(repo, "b.txt", Some(&ContentHash::of(b"changed\n"))).unwrap());
        assert!(head_matches(repo, "b.txt", Some(&ContentHash::of(b"b\n"))).unwrap());
    }
}
