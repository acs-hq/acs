//! `git blame` reads for `acs blame` (WP4.2): the commit behind each line of a file, and the
//! trailers of those commits.
//!
//! The file is blamed as it is in the work tree, so lines that are not committed yet are told
//! apart from committed ones. Commit messages are read in one `git log` for all the commits of
//! the file, not one process per commit.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::GitError;
use crate::refs::git_output;
use crate::trailers::{Trailer, parse_trailers};

/// One line of the blamed file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlameLine {
    /// The line number in the work tree file, from 1.
    pub line: usize,
    /// The commit that last changed the line; `None` when the line is not committed.
    pub commit: Option<String>,
    /// The line's number in that commit's version of the file (the line itself for an
    /// uncommitted line).
    pub original_line: usize,
    /// The line's text, without its line ending.
    pub text: String,
}

/// The result of [`blame_file`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Blame {
    /// One entry per line, in order.
    pub lines: Vec<BlameLine>,
    /// The trailers of each commit named in `lines`, in message order.
    pub trailers: HashMap<String, Vec<Trailer>>,
}

/// Blames `path` (relative to the work tree root) as it is in the work tree. `comment` is git's
/// `core.commentChar`, which decides what counts as a trailer block.
///
/// A file git does not know yet (untracked, or added but never committed) has all its lines
/// uncommitted.
///
/// # Errors
///
/// [`GitError::Command`] when `git` fails and the file is not in the work tree either.
pub fn blame_file(work_tree: &Path, path: &str, comment: char) -> Result<Blame, GitError> {
    let output = git_output(work_tree, &["blame", "--porcelain", "--", path], None, &[])?;
    let lines = if output.status.success() {
        parse_porcelain(&String::from_utf8_lossy(&output.stdout))
    } else {
        // Not in HEAD: everything in the file is new.
        let bytes = std::fs::read(work_tree.join(path)).map_err(|_| GitError::Command {
            args: "blame".to_owned(),
            detail: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        })?;
        String::from_utf8_lossy(&bytes)
            .lines()
            .enumerate()
            .map(|(at, text)| BlameLine {
                line: at + 1,
                commit: None,
                original_line: at + 1,
                text: text.to_owned(),
            })
            .collect()
    };
    let commits: Vec<&str> = {
        let mut seen = HashSet::new();
        lines
            .iter()
            .filter_map(|l| l.commit.as_deref())
            .filter(|id| seen.insert(*id))
            .collect()
    };
    let trailers = trailers_of(work_tree, &commits, comment)?;
    Ok(Blame { lines, trailers })
}

fn is_object_id(word: &str) -> bool {
    matches!(word.len(), 40 | 64) && word.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Parses `git blame --porcelain`: a header line per source line (`<id> <orig> <final> [<n>]`),
/// the commit's metadata on its first appearance, then the line's text after a tab.
fn parse_porcelain(text: &str) -> Vec<BlameLine> {
    let mut lines = Vec::new();
    let mut current: Option<(String, usize, usize)> = None;
    for raw in text.lines() {
        if let Some(text) = raw.strip_prefix('\t') {
            if let Some((id, original, line)) = current.take() {
                let uncommitted = id.bytes().all(|b| b == b'0');
                lines.push(BlameLine {
                    line,
                    commit: (!uncommitted).then_some(id),
                    original_line: original,
                    text: text.to_owned(),
                });
            }
            continue;
        }
        let mut words = raw.split(' ');
        if let (Some(id), Some(original), Some(line)) = (words.next(), words.next(), words.next())
            && is_object_id(id)
            && let (Ok(original), Ok(line)) = (original.parse(), line.parse())
        {
            current = Some((id.to_owned(), original, line));
        }
    }
    lines
}

/// The trailers of each of `commits`, from one `git log --no-walk`.
fn trailers_of(
    work_tree: &Path,
    commits: &[&str],
    comment: char,
) -> Result<HashMap<String, Vec<Trailer>>, GitError> {
    let mut found = HashMap::new();
    if commits.is_empty() {
        return Ok(found);
    }
    let mut args = vec!["log", "--no-walk=unsorted", "--format=%x1e%H%n%B"];
    args.extend_from_slice(commits);
    args.push("--");
    let output = git_output(work_tree, &args, None, &[])?;
    if !output.status.success() {
        return Err(GitError::Command {
            args: "log".to_owned(),
            detail: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }
    let text = String::from_utf8_lossy(&output.stdout);
    for entry in text.split('\x1e').filter(|e| !e.trim().is_empty()) {
        let (id, message) = entry.split_once('\n').unwrap_or((entry, ""));
        found.insert(id.trim().to_owned(), parse_trailers(message, comment));
    }
    Ok(found)
}
