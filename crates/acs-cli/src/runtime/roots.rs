//! Which work tree of the repository a tool call belongs to.
//!
//! A harness reports the directory its session runs in, not the directory a call acts on: a
//! session started in the main checkout can edit a file in a linked worktree
//! (`.claude/worktrees/<name>/src/lib.rs`), or run `cd <worktree> && git commit` there. Claims,
//! sealed globs and snapshots are all keyed by paths relative to a work tree's root, so such a
//! call must be judged in the work tree that holds the files: the file above is `src/lib.rs`, the
//! same file every other checkout of the repository calls `src/lib.rs`.
//!
//! - [`for_event`] picks the work tree for a hook event: the one that holds everything the call
//!   writes, deletes or commits, when that is a single work tree of this repository; otherwise
//!   the session's own.
//! - [`split_by_tree`] does the same per path for `acs claim`, `acs release` and the MCP tools,
//!   which can name files in several work trees at once.
//! - [`git_work_tree`] is the work tree a `git` invocation runs in, for the commit and push
//!   markers the git hooks look up by work tree.
//!
//! A path directly naming a worktree's root (`rm -rf .claude/worktrees/x`) belongs to the work
//! tree around it: removing a worktree is an operation on the checkout that contains it.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use acs_core::gate::{Confidence, GitInvocation, TargetOp};
use acs_core::{ToolEvent, ToolKind};
use acs_git::RepoLayout;
use acs_paths::has_glob_meta;
use acs_shell::analyze;

use super::RepoContext;
use super::paths::dialect_for;

/// The most places of one event examined; a call touching more keeps the session's work tree.
const MAX_PLACES: usize = 256;

/// `/c/Users/me` as Git Bash on Windows writes `C:\Users\me`. `None` for anything else, and on
/// other systems.
#[must_use]
pub fn git_bash_drive(raw: &str) -> Option<String> {
    if !cfg!(windows) {
        return None;
    }
    let bytes = raw.as_bytes();
    let drive = *bytes.get(1)?;
    let rooted = bytes.first() == Some(&b'/') && drive.is_ascii_alphabetic();
    let ends = matches!(bytes.get(2), None | Some(b'/'));
    (rooted && ends).then(|| {
        format!(
            "{}:/{}",
            char::from(drive).to_ascii_uppercase(),
            raw.get(3..).unwrap_or("")
        )
    })
}

/// The filesystem path of `raw` as an event or a command wrote it: absolute, or relative to
/// `cwd`; `.` and `..` are resolved by name (no link is followed).
#[must_use]
pub fn fs_path(raw: &str, cwd: &Path) -> PathBuf {
    let converted = git_bash_drive(raw);
    let joined = cwd.join(converted.as_deref().unwrap_or(raw));
    let mut out = PathBuf::new();
    for part in joined.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// Whether the repository has linked worktrees at all (one directory lookup): without any, every
/// path is in the one work tree and nothing here has work to do.
fn has_linked_worktrees(layout: &RepoLayout) -> bool {
    layout.common_dir.join("worktrees").is_dir()
}

/// Looks up the work tree of this repository that contains a directory, remembering answers.
struct Finder<'a> {
    base: &'a RepoLayout,
    seen: HashMap<PathBuf, Option<RepoLayout>>,
}

impl<'a> Finder<'a> {
    fn new(base: &'a RepoLayout) -> Self {
        Self {
            base,
            seen: HashMap::new(),
        }
    }

    /// The work tree of this repository that contains `dir`; `None` when `dir` is outside every
    /// work tree, or in another repository (a submodule, an unrelated checkout).
    fn tree_of(&mut self, dir: &Path) -> Option<RepoLayout> {
        if let Some(known) = self.seen.get(dir) {
            return known.clone();
        }
        let found = acs_git::discover(dir)
            .ok()
            .filter(|layout| layout.common_dir == self.base.common_dir);
        self.seen.insert(dir.to_path_buf(), found.clone());
        found
    }
}

/// The directory that holds `raw`, a path or a glob pattern.
fn holder(raw: &str, cwd: &Path) -> Option<PathBuf> {
    let literal = if has_glob_meta(raw) {
        let cut = raw.find(['*', '?', '[', '{']).unwrap_or(raw.len());
        // Up to the last separator before the pattern starts; the pattern's own directory.
        match raw[..cut].rfind(['/', '\\']) {
            Some(end) => &raw[..=end],
            None => return Some(cwd.to_path_buf()),
        }
    } else {
        raw
    };
    let path = fs_path(literal, cwd);
    if has_glob_meta(raw) {
        Some(path)
    } else {
        path.parent().map(Path::to_path_buf)
    }
}

/// The directory a `git` invocation runs in; `None` when the analysis could not tell.
fn git_dir(git: &GitInvocation, cwd: &Path) -> Option<PathBuf> {
    (!git.dir_unknown).then(|| fs_path(&git.dir, cwd))
}

/// The work tree `event` is judged in: `base` (the work tree of the event's `cwd`), unless
/// everything the event writes, deletes or commits is in one other work tree of the repository.
/// Read-only tools follow the files they read, so the lore of that work tree applies.
#[must_use]
pub fn for_event(event: &ToolEvent, cwd: &Path, base: RepoLayout) -> RepoLayout {
    let Some(tool) = &event.tool else { return base };
    if !has_linked_worktrees(&base) {
        return base;
    }
    let mut places: Vec<PathBuf> = Vec::new();
    if tool.kind == ToolKind::Shell {
        let command = event.command.as_deref().unwrap_or("");
        for simple in analyze(command, dialect_for(&tool.name)).commands {
            for target in &simple.targets {
                if target.op != TargetOp::Read && target.confidence != Confidence::Partial {
                    places.extend(holder(&target.path, cwd));
                }
            }
            if let Some(git) = &simple.git
                && (git.creates_commit || git.subcommand == "push")
            {
                match git_dir(git, cwd) {
                    Some(dir) => places.push(dir),
                    // Somewhere we cannot see: stay with the session's work tree.
                    None => return base,
                }
            }
        }
    } else {
        places.extend(event.paths.iter().filter_map(|raw| holder(raw, cwd)));
    }
    if places.len() > MAX_PLACES {
        return base;
    }
    let mut finder = Finder::new(&base);
    let mut chosen: Option<RepoLayout> = None;
    for place in &places {
        let Some(tree) = finder.tree_of(place) else {
            continue; // Outside the repository under any root.
        };
        match &chosen {
            Some(first) if first.work_tree != tree.work_tree => return base,
            Some(_) => {}
            None => chosen = Some(tree),
        }
    }
    chosen.unwrap_or(base)
}

/// The work tree a `git` invocation of a shell event runs in (after `cd` and `-C`), when it is a
/// work tree of this repository; the context's own otherwise, and when it cannot be determined.
#[must_use]
pub fn git_work_tree(ctx: &RepoContext, git: &GitInvocation, cwd: &Path) -> PathBuf {
    git_dir(git, cwd)
        .filter(|_| has_linked_worktrees(&ctx.layout))
        .and_then(|dir| Finder::new(&ctx.layout).tree_of(&dir))
        .map_or_else(|| ctx.layout.work_tree.clone(), |tree| tree.work_tree)
}

/// Raw paths (relative to `cwd` or absolute) split by work tree: the ones to resolve in `ctx`'s
/// own work tree, unchanged, and the ones in other work trees of the repository, made absolute
/// and grouped by tree in order of first appearance.
#[must_use]
pub fn split_by_tree(
    ctx: &RepoContext,
    paths: &[String],
    cwd: &Path,
) -> (Vec<String>, Vec<(RepoLayout, Vec<String>)>) {
    if !has_linked_worktrees(&ctx.layout) {
        return (paths.to_vec(), Vec::new());
    }
    let mut finder = Finder::new(&ctx.layout);
    let mut here = Vec::new();
    let mut elsewhere: Vec<(RepoLayout, Vec<String>)> = Vec::new();
    for raw in paths {
        let tree = holder(raw, cwd).and_then(|dir| finder.tree_of(&dir));
        match tree {
            Some(tree) if tree.work_tree != ctx.layout.work_tree => {
                let absolute = fs_path(raw, cwd).to_string_lossy().into_owned();
                match elsewhere
                    .iter_mut()
                    .find(|(t, _)| t.work_tree == tree.work_tree)
                {
                    Some((_, group)) => group.push(absolute),
                    None => elsewhere.push((tree, vec![absolute])),
                }
            }
            _ => here.push(raw.clone()),
        }
    }
    (here, elsewhere)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dots_are_resolved_by_name() {
        let cwd = if cfg!(windows) {
            Path::new("C:\\work\\repo")
        } else {
            Path::new("/work/repo")
        };
        assert_eq!(fs_path("a/./b/../c.rs", cwd), cwd.join("a").join("c.rs"));
        assert_eq!(fs_path("..", cwd), cwd.parent().unwrap());
        assert_eq!(fs_path("", cwd), cwd);
    }

    #[test]
    fn git_bash_drive_paths_are_windows_paths_on_windows_only() {
        if cfg!(windows) {
            assert_eq!(
                git_bash_drive("/c/Users/me").as_deref(),
                Some("C:/Users/me")
            );
            assert_eq!(git_bash_drive("/d").as_deref(), Some("D:/"));
            assert_eq!(
                fs_path("/c/work/x.rs", Path::new("D:\\other")),
                Path::new("C:\\work\\x.rs")
            );
        } else {
            assert_eq!(git_bash_drive("/c/Users/me"), None);
        }
        assert_eq!(git_bash_drive("/usr/bin"), None);
        assert_eq!(git_bash_drive("c/x"), None);
        assert_eq!(git_bash_drive("/"), None);
    }

    #[test]
    fn a_path_is_held_by_its_directory_and_a_pattern_by_its_literal_part() {
        let cwd = if cfg!(windows) {
            Path::new("C:\\work\\repo")
        } else {
            Path::new("/work/repo")
        };
        assert_eq!(holder("src/lib.rs", cwd).unwrap(), cwd.join("src"));
        assert_eq!(
            holder("wt/src/*.rs", cwd).unwrap(),
            cwd.join("wt").join("src")
        );
        assert_eq!(holder("*.log", cwd).unwrap(), cwd);
        // A directory named directly belongs to the tree around it.
        assert_eq!(holder("wt", cwd).unwrap(), cwd);
        assert_eq!(holder(".", cwd).unwrap(), cwd.parent().unwrap());
    }
}
