//! Turning what an event touches into the engine's [`Target`]s.
//!
//! This is step 3 of the pipeline (SPEC section 9) plus the shell expansion the engine relies on
//! (decision 0004 rule 5, handoff section 3):
//!
//! - an absolute `cwd` goes to the resolver (a relative one would be unresolvable);
//! - `resolve` is used for writes, `resolve_entry` for the things `rm` and `mv` remove (a link in
//!   the last component is the thing deleted), `resolve_recursive_report` for directories a
//!   recursive operation enters (its `truncated` flag becomes the expansion's);
//! - every recursive target and every glob pattern is expanded into an [`Expansion`], including
//!   the repository root (`rm -rf .`), so a recursive delete is never "unlisted" by accident;
//! - `Partial` shell targets are dropped here: the engine still asks about a destructive command
//!   with unknown targets through [`ShellAnalysis::has_unknown_targets`].

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use acs_core::gate::{Confidence, Expansion, PathLocation, ShellAnalysis, Target, TargetOp};
use acs_core::{RepoPath, ToolEvent, ToolKind};
use acs_paths::has_glob_meta;
use acs_shell::{Dialect, analyze};

use super::RepoContext;

/// How many directory entries a recursive walk examines before it reports truncation.
const WALK_CAP: usize = 20_000;
/// The most files one write expansion lists (and so claims) before a warning is raised.
pub const WRITE_EXPANSION_CAP: usize = 500;
/// The most predicted paths remembered for the post-tool diff.
pub const MAX_PREDICTED: usize = 2_000;

/// What the pipeline needs to know about the paths of one event.
#[derive(Debug, Default, Clone)]
pub struct Normalized {
    /// The targets for the engine.
    pub targets: Vec<Target>,
    /// The shell analysis, for shell events.
    pub shell: Option<ShellAnalysis>,
    /// Every repository path the event writes or deletes (targets, expansions and paths reached
    /// through links), each once, for the claims snapshot.
    pub repo_paths: Vec<RepoPath>,
    /// The same paths as strings, capped, for the post-tool diff.
    pub predicted: Vec<String>,
    /// Directories to list in the post-tool scan: where the command ran and where its targets
    /// live (`""` is the repository root).
    pub scope_dirs: Vec<String>,
    /// Observations the agent should hear about (a truncated expansion).
    pub warnings: Vec<String>,
    /// Whether a shell command in the event creates a git commit.
    pub creates_commit: bool,
}

/// Options of [`normalize`].
#[derive(Debug, Clone, Copy, Default)]
pub struct NormalizeOptions {
    /// Write targets that are directories list the files under them (used when claiming a
    /// directory). Otherwise they are checked for sealing only.
    pub expand_write_dirs: bool,
}

/// The shell dialect for a tool name (decision 0011 section 3): a name containing `powershell`
/// or `pwsh` is PowerShell, one containing `cmd` is cmd, everything else is bash.
#[must_use]
pub fn dialect_for(tool_name: &str) -> Dialect {
    let name = tool_name.to_ascii_lowercase();
    if name.contains("powershell") || name.contains("pwsh") {
        Dialect::PowerShell
    } else if name.contains("cmd") {
        Dialect::Cmd
    } else {
        Dialect::Bash
    }
}

/// The absolute working directory of an event; a relative `cwd` is taken relative to the
/// process's own.
#[must_use]
pub fn absolute_cwd(raw: &str) -> PathBuf {
    std::path::absolute(raw).unwrap_or_else(|_| PathBuf::from(raw))
}

/// Normalizes the paths of `event`.
#[must_use]
pub fn normalize(ctx: &RepoContext, event: &ToolEvent, options: NormalizeOptions) -> Normalized {
    let cwd = absolute_cwd(&event.cwd);
    let policy = ctx.policy_view();
    let destructive = policy.destructive();
    let delete_cap = (destructive
        .max_files_deleted_per_command
        .max(destructive.max_files_deleted_outside_claims) as usize)
        .saturating_add(1);
    let mut builder = Builder {
        ctx,
        cwd,
        delete_cap,
        options,
        out: Normalized::default(),
    };
    let Some(tool) = &event.tool else {
        return builder.finish();
    };
    match tool.kind {
        ToolKind::Read => {}
        ToolKind::Write | ToolKind::Edit | ToolKind::Other => {
            for raw in &event.paths {
                builder.write_path(raw);
            }
        }
        ToolKind::Delete => {
            for raw in &event.paths {
                builder.delete_path(raw);
            }
        }
        ToolKind::Move => match event.paths.split_last() {
            Some((destination, sources)) if !sources.is_empty() => {
                for raw in sources {
                    builder.delete_path(raw);
                }
                builder.write_path(destination);
            }
            // One path and no destination: treat what we cannot see as the destructive case.
            Some((only, _)) => builder.delete_path(only),
            None => {}
        },
        ToolKind::Shell => builder.shell(&tool.name, event.command.as_deref().unwrap_or("")),
    }
    builder.finish()
}

struct Builder<'a> {
    ctx: &'a RepoContext,
    cwd: PathBuf,
    delete_cap: usize,
    options: NormalizeOptions,
    out: Normalized,
}

impl Builder<'_> {
    fn is_real_dir(&self, rel: &RepoPath) -> bool {
        fs::symlink_metadata(self.ctx.layout.work_tree.join(rel.as_str()))
            .is_ok_and(|meta| meta.is_dir())
    }

    fn write_path(&mut self, raw: &str) {
        let resolved = self.ctx.resolver.resolve(raw, &self.cwd);
        let is_dir = resolved
            .repo_path()
            .is_some_and(|rel| self.is_real_dir(rel));
        if !is_dir {
            self.out.targets.push(Target {
                path: resolved,
                op: TargetOp::Write,
                recursive: false,
                expansion: None,
            });
            return;
        }
        let report = self
            .ctx
            .resolver
            .resolve_recursive_report(raw, &self.cwd, WALK_CAP);
        let mut expansion = if self.options.expand_write_dirs {
            self.ctx
                .resolver
                .expand_recursive(&report.path, WRITE_EXPANSION_CAP)
        } else {
            Expansion::default()
        };
        expansion.truncated |= self.options.expand_write_dirs && report.truncated;
        self.note_truncated_write(raw, &expansion);
        self.out.targets.push(Target {
            path: report.path,
            op: TargetOp::Write,
            recursive: true,
            expansion: Some(expansion),
        });
    }

    fn delete_path(&mut self, raw: &str) {
        let entry = self.ctx.resolver.resolve_entry(raw, &self.cwd);
        let is_dir = match &entry.location {
            PathLocation::InRepo(rel) => self.is_real_dir(rel),
            PathLocation::Outside(path) => Path::new(path).is_dir(),
            PathLocation::Unresolvable(_) => false,
        };
        if !is_dir {
            self.out.targets.push(Target {
                path: entry,
                op: TargetOp::Delete,
                recursive: false,
                expansion: None,
            });
            return;
        }
        let report = self
            .ctx
            .resolver
            .resolve_recursive_report(raw, &self.cwd, WALK_CAP);
        let mut expansion = self
            .ctx
            .resolver
            .expand_recursive(&report.path, self.delete_cap);
        expansion.truncated |= report.truncated;
        self.out.targets.push(Target {
            path: report.path,
            op: TargetOp::Delete,
            recursive: true,
            expansion: Some(expansion),
        });
    }

    fn pattern(&mut self, raw: &str, op: TargetOp, recursive: bool) {
        let cap = if op == TargetOp::Delete {
            self.delete_cap
        } else {
            WRITE_EXPANSION_CAP
        };
        let expansion = self
            .ctx
            .resolver
            .expand_glob(raw, &self.cwd, recursive, cap);
        if op != TargetOp::Delete {
            self.note_truncated_write(raw, &expansion);
        }
        let path = if op == TargetOp::Delete {
            self.ctx.resolver.resolve_entry(raw, &self.cwd)
        } else {
            self.ctx.resolver.resolve(raw, &self.cwd)
        };
        self.out.targets.push(Target {
            path,
            op,
            recursive,
            expansion: Some(expansion),
        });
    }

    fn note_truncated_write(&mut self, raw: &str, expansion: &Expansion) {
        if expansion.truncated {
            self.out.warnings.push(format!(
                "`{raw}` reaches more than {WRITE_EXPANSION_CAP} files; only the first \
                 {WRITE_EXPANSION_CAP} were claimed. Name the files you change."
            ));
        }
    }

    fn shell(&mut self, tool_name: &str, command: &str) {
        let analysis = analyze(command, dialect_for(tool_name));
        for simple in &analysis.commands {
            if simple.git.as_ref().is_some_and(|git| git.creates_commit) {
                self.out.creates_commit = true;
            }
            for target in &simple.targets {
                if target.op == TargetOp::Read || target.confidence == Confidence::Partial {
                    continue;
                }
                let raw = target.path.as_str();
                if target.confidence == Confidence::Pattern || has_glob_meta(raw) {
                    self.pattern(raw, target.op, target.recursive);
                } else if target.op == TargetOp::Delete {
                    self.delete_path(raw);
                } else {
                    self.write_path(raw);
                }
            }
        }
        self.out.shell = Some(analysis);
    }

    fn finish(mut self) -> Normalized {
        let mut seen: BTreeSet<RepoPath> = BTreeSet::new();
        let mut paths: Vec<RepoPath> = Vec::new();
        let mut add = |path: &RepoPath, paths: &mut Vec<RepoPath>| {
            if seen.insert(path.clone()) {
                paths.push(path.clone());
            }
        };
        for target in &self.out.targets {
            if target.op == TargetOp::Read {
                continue;
            }
            match (&target.path.location, &target.expansion) {
                (PathLocation::InRepo(_), Some(expansion)) => {
                    for file in &expansion.files {
                        add(file, &mut paths);
                    }
                }
                (PathLocation::InRepo(path), None) => add(path, &mut paths),
                _ => {}
            }
            for linked in &target.path.linked_into_repo {
                add(linked, &mut paths);
            }
        }
        let mut dirs: BTreeSet<String> = BTreeSet::new();
        if let PathLocation::InRepo(rel) = self
            .ctx
            .resolver
            .resolve(&self.cwd.to_string_lossy(), &self.cwd)
            .location
        {
            dirs.insert(dir_key(rel.as_str()));
        }
        for path in paths.iter().take(MAX_PREDICTED) {
            dirs.insert(parent_dir(path.as_str()));
        }
        self.out.predicted = paths
            .iter()
            .take(MAX_PREDICTED)
            .map(|p| p.as_str().to_owned())
            .collect();
        self.out.repo_paths = paths;
        self.out.scope_dirs = dirs.into_iter().take(MAX_SCOPE_DIRS).collect();
        self.out
    }
}

/// The most directories remembered for the post-tool scan.
const MAX_SCOPE_DIRS: usize = 64;

/// The directory of a repository path (`""` for the root).
#[must_use]
pub fn parent_dir(path: &str) -> String {
    path.rsplit_once('/')
        .map_or_else(String::new, |(dir, _)| dir.to_owned())
}

fn dir_key(rel: &str) -> String {
    if rel == "." {
        String::new()
    } else {
        rel.to_owned()
    }
}

/// Resolves raw paths given to the CLI or MCP (`acs claim <path>...`): repository paths of the
/// ones inside the repository, and the raw text of the ones that are not.
#[must_use]
pub fn resolve_for_claim(
    ctx: &RepoContext,
    raw_paths: &[String],
    cwd: &Path,
) -> (Vec<RepoPath>, Vec<String>) {
    let mut inside = Vec::new();
    let mut outside = Vec::new();
    for raw in raw_paths {
        match ctx.resolver.resolve(raw, cwd).location {
            PathLocation::InRepo(path) => inside.push(path),
            PathLocation::Outside(_) | PathLocation::Unresolvable(_) => outside.push(raw.clone()),
        }
    }
    (inside, outside)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dialect_follows_the_tool_name() {
        assert_eq!(dialect_for("Bash"), Dialect::Bash);
        assert_eq!(dialect_for("run_shell_command"), Dialect::Bash);
        assert_eq!(dialect_for("PowerShell"), Dialect::PowerShell);
        assert_eq!(dialect_for("pwsh"), Dialect::PowerShell);
        assert_eq!(dialect_for("Cmd"), Dialect::Cmd);
    }

    #[test]
    fn parent_directories() {
        assert_eq!(parent_dir("a/b/c.rs"), "a/b");
        assert_eq!(parent_dir("c.rs"), "");
        assert_eq!(dir_key("."), "");
        assert_eq!(dir_key("src"), "src");
    }
}
