//! Local repository reads: layout, branch, remotes, pilot email, index blobs.
//!
//! Everything on the hook hot path here reads a handful of small files; only [`pilot_email`] and
//! [`index_blobs`] start `git`, and the hook calls them only when a seat is created or a shell
//! command changed tracked files.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::GitError;

/// Where a work tree and its git data live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoLayout {
    /// The canonical root of the work tree (the directory that holds `.git`).
    pub work_tree: PathBuf,
    /// This work tree's git directory: `.git` itself, or `.git/worktrees/<name>` for a linked
    /// worktree. `HEAD` and `index` live here.
    pub git_dir: PathBuf,
    /// The common git directory shared by all worktrees of the repository: refs, objects and
    /// `config` live here. Equal to `git_dir` for the main work tree.
    pub common_dir: PathBuf,
}

/// Finds the work tree containing `start`.
///
/// Walks up from `start` (which may not exist any more; the nearest existing ancestor is used)
/// looking for a `.git` directory, or a `.git` file (`gitdir: <path>`) as linked worktrees and
/// submodules have. `GIT_DIR` and friends are ignored on purpose, as in
/// `acs_store::find_common_dir`: a call made from inside a git hook must still describe the
/// repository that contains `start`.
///
/// # Errors
///
/// [`GitError::NotAWorkTree`] when no ancestor has a `.git`, [`GitError::Io`] when `.git` cannot
/// be read.
pub fn discover(start: &Path) -> Result<RepoLayout, GitError> {
    let start =
        std::path::absolute(start).map_err(|e| GitError::io("resolving", start.to_path_buf(), e))?;
    for dir in start.ancestors() {
        let dot_git = dir.join(".git");
        let Ok(meta) = fs::symlink_metadata(&dot_git) else {
            continue;
        };
        let git_dir = if meta.is_dir() {
            dot_git
        } else if meta.is_file() {
            let text = fs::read_to_string(&dot_git)
                .map_err(|e| GitError::io("reading", dot_git.clone(), e))?;
            let Some(target) = text
                .lines()
                .find_map(|line| line.trim().strip_prefix("gitdir:"))
                .map(str::trim)
            else {
                continue;
            };
            let target = Path::new(target);
            if target.is_absolute() {
                target.to_path_buf()
            } else {
                dir.join(target)
            }
        } else {
            continue;
        };
        let git_dir = dunce::canonicalize(&git_dir)
            .map_err(|e| GitError::io("resolving", git_dir.clone(), e))?;
        let common_dir = match fs::read_to_string(git_dir.join("commondir")) {
            Ok(text) => {
                let rel = Path::new(text.trim());
                let joined = if rel.is_absolute() {
                    rel.to_path_buf()
                } else {
                    git_dir.join(rel)
                };
                dunce::canonicalize(&joined).map_err(|e| GitError::io("resolving", joined, e))?
            }
            Err(_) => git_dir.clone(),
        };
        let work_tree =
            dunce::canonicalize(dir).map_err(|e| GitError::io("resolving", dir.to_path_buf(), e))?;
        return Ok(RepoLayout {
            work_tree,
            git_dir,
            common_dir,
        });
    }
    Err(GitError::NotAWorkTree(start))
}

/// The checked-out branch, read straight from `HEAD` in `git_dir`. `None` when HEAD is detached
/// or cannot be read. An unborn branch (no commits yet) is reported by name.
#[must_use]
pub fn current_branch(git_dir: &Path) -> Option<String> {
    let head = fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let target = head.trim().strip_prefix("ref:")?.trim();
    Some(
        target
            .strip_prefix("refs/heads/")
            .unwrap_or(target)
            .to_owned(),
    )
}

/// Whether the repository has a remote configured: a `[remote "..."]` section in the common
/// directory's `config` file. Does not follow `include` directives (a remote defined only in an
/// included file is missed, which keeps ACS in local-only mode).
#[must_use]
pub fn has_remote(common_dir: &Path) -> bool {
    fs::read_to_string(common_dir.join("config")).is_ok_and(|text| {
        text.lines()
            .any(|line| line.trim_start().starts_with("[remote \""))
    })
}

/// The pilot's email: `git config user.email` in `work_tree`, trimmed. `None` when unset, empty
/// or `git` cannot run. Starts a process: call it only when a seat is created.
#[must_use]
pub fn pilot_email(work_tree: &Path) -> Option<String> {
    let output = git_command(work_tree)
        .args(["config", "--get", "user.email"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let email = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!email.is_empty()).then_some(email)
}

/// The pilot identity used when `git config user.email` is unset: `<user>@localhost`, where
/// `<user>` is `$USER` or `$USERNAME`, or `unknown`. It is stable for the machine and user, so
/// the same person keeps one identity, and it is not a deliverable address.
#[must_use]
pub fn fallback_pilot() -> String {
    let user = ["USER", "USERNAME"]
        .iter()
        .find_map(|name| std::env::var(name).ok())
        .map(|name| name.trim().to_lowercase())
        .filter(|name| !name.is_empty() && !name.contains(char::is_whitespace))
        .unwrap_or_else(|| "unknown".to_owned());
    format!("{user}@localhost")
}

/// The pilot's email, or [`fallback_pilot`] when git has none configured.
#[must_use]
pub fn pilot_email_or_fallback(work_tree: &Path) -> String {
    pilot_email(work_tree).unwrap_or_else(fallback_pilot)
}

/// The number of entries in the work tree's index, from its 12-byte header; `None` when there is
/// no index or it is not readable. Cheap: used to decide how thorough a working-tree scan can
/// afford to be.
#[must_use]
pub fn index_entry_count(git_dir: &Path) -> Option<u32> {
    let mut header = [0u8; 12];
    fs::File::open(git_dir.join("index"))
        .ok()?
        .read_exact(&mut header)
        .ok()?;
    if &header[..4] != b"DIRC" {
        return None;
    }
    Some(u32::from_be_bytes([
        header[8], header[9], header[10], header[11],
    ]))
}

/// Largest blob [`index_blobs`] returns, in bytes. Bigger ones are reported as absent.
pub const MAX_INDEX_BLOB: usize = 16 * 1024 * 1024;

/// The staged content (`:path`) of each path, via one `git cat-file --batch` process. A path that
/// is not in the index, is larger than [`MAX_INDEX_BLOB`] or contains a newline yields `None`.
/// The result has one entry per input path, in order.
///
/// # Errors
///
/// [`GitError::Command`] when `git` cannot be run or its output cannot be read.
pub fn index_blobs(work_tree: &Path, paths: &[String]) -> Result<Vec<Option<Vec<u8>>>, GitError> {
    let fail = |detail: String| GitError::Command {
        args: "cat-file --batch".to_owned(),
        detail,
    };
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let mut child = git_command(work_tree)
        .args(["cat-file", "--batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| fail(e.to_string()))?;
    let mut input = String::new();
    for path in paths {
        if path.contains('\n') {
            // Keep the line count aligned: an unresolvable name gets a "missing" answer.
            input.push_str(":\n");
        } else {
            input.push(':');
            input.push_str(path);
            input.push('\n');
        }
    }
    if let Some(mut stdin) = child.stdin.take() {
        // The input is small (a few paths), far below a pipe buffer, so writing it all before
        // reading cannot deadlock.
        stdin
            .write_all(input.as_bytes())
            .map_err(|e| fail(e.to_string()))?;
    }
    let mut out = Vec::new();
    if let Some(mut stdout) = child.stdout.take() {
        stdout
            .read_to_end(&mut out)
            .map_err(|e| fail(e.to_string()))?;
    }
    let _ = child.wait();
    parse_batch(&out, paths.len()).map_err(fail)
}

/// Parses `git cat-file --batch` output for `count` requests.
fn parse_batch(mut out: &[u8], count: usize) -> Result<Vec<Option<Vec<u8>>>, String> {
    let mut blobs = Vec::with_capacity(count);
    for _ in 0..count {
        let Some(newline) = out.iter().position(|&b| b == b'\n') else {
            return Err("truncated output".to_owned());
        };
        let header = String::from_utf8_lossy(&out[..newline]).into_owned();
        out = &out[newline + 1..];
        let mut words = header.split(' ');
        let (_, kind, size) = (words.next(), words.next(), words.next());
        if let (Some("blob"), Some(size)) = (kind, size) {
            let size: usize = size.parse().map_err(|_| format!("bad header {header:?}"))?;
            if out.len() < size + 1 {
                return Err("truncated blob".to_owned());
            }
            let content = &out[..size];
            blobs.push((size <= MAX_INDEX_BLOB).then(|| content.to_vec()));
            out = &out[size + 1..];
        } else {
            blobs.push(None);
        }
    }
    Ok(blobs)
}

/// A `git` command for `work_tree` with the `GIT_DIR` family removed from its environment, so a
/// call made from inside a git hook still acts on `work_tree`, and with optional locks disabled
/// so reading never contends with the user's own git commands.
pub(crate) fn git_command(work_tree: &Path) -> Command {
    let mut command = Command::new("git");
    command.arg("-C").arg(work_tree);
    for var in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_PREFIX",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    ] {
        command.env_remove(var);
    }
    command.env("GIT_OPTIONAL_LOCKS", "0");
    command
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

    fn init(dir: &Path) {
        git(dir, &["init", "-q", "-b", "main", "."]);
    }

    #[test]
    fn discovers_the_work_tree_from_a_subdirectory() {
        let tmp = tempfile::tempdir().unwrap();
        init(tmp.path());
        fs::create_dir_all(tmp.path().join("a/b")).unwrap();
        let layout = discover(&tmp.path().join("a/b")).unwrap();
        let root = dunce::canonicalize(tmp.path()).unwrap();
        assert_eq!(layout.work_tree, root);
        assert_eq!(layout.git_dir, root.join(".git"));
        assert_eq!(layout.common_dir, root.join(".git"));
        // A directory that does not exist yet resolves through its nearest existing ancestor.
        assert_eq!(
            discover(&tmp.path().join("a/not/there")).unwrap().work_tree,
            root
        );
    }

    #[test]
    fn outside_a_repository_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(matches!(
            discover(tmp.path()),
            Err(GitError::NotAWorkTree(_))
        ));
    }

    #[test]
    fn a_linked_worktree_has_its_own_git_dir_and_the_shared_common_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let main = tmp.path().join("main");
        fs::create_dir(&main).unwrap();
        init(&main);
        git(&main, &["commit", "-q", "--allow-empty", "-m", "init"]);
        let linked = tmp.path().join("linked");
        git(
            &main,
            &["worktree", "add", "-q", "-b", "feature", linked.to_str().unwrap()],
        );
        let layout = discover(&linked).unwrap();
        assert_eq!(layout.work_tree, dunce::canonicalize(&linked).unwrap());
        let main_git = dunce::canonicalize(main.join(".git")).unwrap();
        assert_eq!(layout.common_dir, main_git);
        assert_ne!(layout.git_dir, main_git);
        assert_eq!(current_branch(&layout.git_dir).as_deref(), Some("feature"));
        assert_eq!(
            current_branch(&main_git).as_deref(),
            Some("main"),
            "the main work tree keeps its own HEAD"
        );
    }

    #[test]
    fn branch_is_read_from_head_including_unborn_and_detached() {
        let tmp = tempfile::tempdir().unwrap();
        init(tmp.path());
        let git_dir = tmp.path().join(".git");
        assert_eq!(current_branch(&git_dir).as_deref(), Some("main"), "unborn");
        fs::write(git_dir.join("HEAD"), "ref: refs/heads/feature/x\n").unwrap();
        assert_eq!(current_branch(&git_dir).as_deref(), Some("feature/x"));
        fs::write(
            git_dir.join("HEAD"),
            "0123456789012345678901234567890123456789\n",
        )
        .unwrap();
        assert_eq!(current_branch(&git_dir), None, "detached");
        assert_eq!(current_branch(&tmp.path().join("nope")), None);
    }

    #[test]
    fn remotes_are_detected_from_the_config() {
        let tmp = tempfile::tempdir().unwrap();
        init(tmp.path());
        let git_dir = tmp.path().join(".git");
        assert!(!has_remote(&git_dir));
        git(
            tmp.path(),
            &["remote", "add", "origin", "https://example.com/r.git"],
        );
        assert!(has_remote(&git_dir));
    }

    #[test]
    fn pilot_email_comes_from_git_config() {
        let tmp = tempfile::tempdir().unwrap();
        init(tmp.path());
        git(tmp.path(), &["config", "user.email", "sara@example.com"]);
        assert_eq!(pilot_email(tmp.path()).as_deref(), Some("sara@example.com"));
        assert_eq!(pilot_email_or_fallback(tmp.path()), "sara@example.com");
        assert!(fallback_pilot().ends_with("@localhost"));
    }

    #[test]
    fn index_header_gives_the_entry_count() {
        let tmp = tempfile::tempdir().unwrap();
        init(tmp.path());
        let git_dir = tmp.path().join(".git");
        assert_eq!(index_entry_count(&git_dir), None, "no index yet");
        fs::write(tmp.path().join("a.txt"), "a").unwrap();
        fs::write(tmp.path().join("b.txt"), "b").unwrap();
        git(tmp.path(), &["add", "."]);
        assert_eq!(index_entry_count(&git_dir), Some(2));
    }

    #[test]
    fn staged_blobs_are_read_in_order_with_misses_as_none() {
        let tmp = tempfile::tempdir().unwrap();
        init(tmp.path());
        fs::write(tmp.path().join("a.txt"), "staged a\n").unwrap();
        git(tmp.path(), &["add", "a.txt"]);
        fs::write(tmp.path().join("a.txt"), "changed in the work tree\n").unwrap();
        let blobs = index_blobs(
            tmp.path(),
            &["a.txt".to_owned(), "missing.txt".to_owned(), "a.txt".to_owned()],
        )
        .unwrap();
        assert_eq!(blobs[0].as_deref(), Some(&b"staged a\n"[..]));
        assert_eq!(blobs[1], None);
        assert_eq!(blobs[2].as_deref(), Some(&b"staged a\n"[..]));
        assert!(index_blobs(tmp.path(), &[]).unwrap().is_empty());
    }

    #[test]
    fn batch_output_parsing_rejects_truncation() {
        assert!(parse_batch(b"abc blob 5\nhel", 1).is_err());
        assert_eq!(
            parse_batch(b"abc blob 3\nhey\n:x missing\n", 2).unwrap(),
            vec![Some(b"hey".to_vec()), None]
        );
    }
}
