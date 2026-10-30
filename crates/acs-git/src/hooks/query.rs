//! The reads the git hooks need, made from inside a hook.
//!
//! Unlike [`crate::repo`], these keep the `GIT_*` environment git set for the hook. This matters
//! for `GIT_INDEX_FILE`: with `git commit -a` or `git commit <paths>`, git runs the hooks against a
//! temporary index, and the staged files are the ones in *that* index.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use crate::GitError;
use crate::trailers::DEFAULT_COMMENT_CHAR;

/// The largest staged blob [`blob_contents`] returns. Bigger ones are reported as absent.
pub const MAX_BLOB: usize = 16 * 1024 * 1024;

/// One path of the staged diff (`git diff --cached`, renames split into a delete and an add).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedChange {
    /// The path, relative to the work tree root.
    pub path: String,
    /// git's status letter: `A`, `M`, `D`, `T` (type change), `U` or `X`.
    pub status: char,
    /// The object id of the staged blob; `None` for a deletion.
    pub oid: Option<String>,
}

impl StagedChange {
    /// Whether the change removes the path.
    #[must_use]
    pub fn is_deletion(&self) -> bool {
        self.status == 'D'
    }
}

fn git(work_tree: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .current_dir(work_tree)
        .env("GIT_OPTIONAL_LOCKS", "0");
    command
}

fn failed(args: &str, detail: impl ToString) -> GitError {
    GitError::Command {
        args: args.to_owned(),
        detail: detail.to_string(),
    }
}

/// The staged changes of the index git is committing (honours `GIT_INDEX_FILE`). Deletions are
/// included, and a rename shows as its old path (deleted) and its new path (added).
///
/// # Errors
///
/// [`GitError::Command`] when `git` cannot be run or fails.
pub fn staged_changes(work_tree: &Path) -> Result<Vec<StagedChange>, GitError> {
    let args = "diff --cached --raw -z";
    let output = git(work_tree)
        .args([
            "diff",
            "--cached",
            "--raw",
            "-z",
            "--no-renames",
            "--no-abbrev",
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| failed(args, e))?;
    if !output.status.success() {
        return Err(failed(args, String::from_utf8_lossy(&output.stderr).trim()));
    }
    let mut tokens = output.stdout.split(|&b| b == 0);
    let mut changes = Vec::new();
    while let Some(meta) = tokens.next() {
        let Some(meta) = meta.strip_prefix(b":") else {
            continue;
        };
        let Some(path) = tokens.next() else { break };
        // `<old mode> <new mode> <old oid> <new oid> <status>`
        let meta = String::from_utf8_lossy(meta);
        let fields: Vec<&str> = meta.split(' ').collect();
        let (Some(&new_oid), Some(&status)) = (fields.get(3), fields.get(4)) else {
            continue;
        };
        let status = status.chars().next().unwrap_or('X');
        let oid =
            (status != 'D' && !new_oid.bytes().all(|b| b == b'0')).then(|| new_oid.to_owned());
        changes.push(StagedChange {
            path: String::from_utf8_lossy(path).into_owned(),
            status,
            oid,
        });
    }
    Ok(changes)
}

/// The content of each blob (by object id) via one `git cat-file --batch`. A missing object or one
/// over [`MAX_BLOB`] bytes yields `None`. One entry per input, in order.
///
/// # Errors
///
/// [`GitError::Command`] when `git` cannot be run or its output cannot be read.
pub fn blob_contents(work_tree: &Path, oids: &[String]) -> Result<Vec<Option<Vec<u8>>>, GitError> {
    let args = "cat-file --batch";
    if oids.is_empty() {
        return Ok(Vec::new());
    }
    let mut child = git(work_tree)
        .args(["cat-file", "--batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| failed(args, e))?;
    let mut input = String::new();
    for oid in oids {
        input.push_str(oid);
        input.push('\n');
    }
    // A few object ids are far below a pipe buffer, so writing them all first cannot deadlock.
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(input.as_bytes())
            .map_err(|e| failed(args, e))?;
    }
    let mut out = Vec::new();
    if let Some(mut stdout) = child.stdout.take() {
        stdout.read_to_end(&mut out).map_err(|e| failed(args, e))?;
    }
    let _ = child.wait();
    let mut rest = out.as_slice();
    let mut blobs = Vec::with_capacity(oids.len());
    for _ in oids {
        let Some(newline) = rest.iter().position(|&b| b == b'\n') else {
            return Err(failed(args, "truncated output"));
        };
        let header = String::from_utf8_lossy(&rest[..newline]).into_owned();
        rest = &rest[newline + 1..];
        let mut words = header.split(' ');
        let (_, kind, size) = (words.next(), words.next(), words.next());
        match (kind, size.and_then(|s| s.parse::<usize>().ok())) {
            (Some("blob"), Some(size)) => {
                if rest.len() < size + 1 {
                    return Err(failed(args, "truncated blob"));
                }
                blobs.push((size <= MAX_BLOB).then(|| rest[..size].to_vec()));
                rest = &rest[size + 1..];
            }
            _ => blobs.push(None),
        }
    }
    Ok(blobs)
}

/// Whether `ancestor` is an ancestor of (or equal to) `descendant`. `None` when git cannot tell,
/// for example because one of the objects is unknown locally.
#[must_use]
pub fn is_ancestor(work_tree: &Path, ancestor: &str, descendant: &str) -> Option<bool> {
    let status = git(work_tree)
        .args(["merge-base", "--is-ancestor", ancestor, descendant])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok()?;
    match status.code()? {
        0 => Some(true),
        1 => Some(false),
        _ => None,
    }
}

/// The committer time (seconds since the epoch) of the newest commit reachable from `HEAD` that
/// touched `path`; `None` when no commit did, or there is no `HEAD` yet.
#[must_use]
pub fn last_commit_time(work_tree: &Path, path: &str) -> Option<i64> {
    let output = git(work_tree)
        .env("GIT_LITERAL_PATHSPECS", "1")
        .args(["log", "-1", "--format=%ct", "HEAD", "--", path])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

/// The comment character of commit messages: `core.commentChar` when it is one character other
/// than `auto`, else `#`.
#[must_use]
pub fn comment_char(work_tree: &Path) -> char {
    let output = git(work_tree)
        .args(["config", "--get", "core.commentChar"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok();
    output
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .filter(|value| value.chars().count() == 1)
        .and_then(|value| value.chars().next())
        .unwrap_or(DEFAULT_COMMENT_CHAR)
}
