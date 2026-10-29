//! The resolver: repository facts (root, git directory, filesystem case sensitivity) and the
//! component-by-component walk that turns a raw path into a [`ResolvedPath`].

use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use acs_core::RepoPath;
use acs_core::gate::{PathLocation, ResolvedPath};

use crate::names::{
    canonical, flip_case, is_windows_device, names_eq, on_disk_name, unverbatim, windows_name,
};

/// How many links one resolution may follow before it is declared a loop (Linux allows 40).
const MAX_LINK_HOPS: usize = 40;

/// Why a [`PathResolver`] could not be built.
#[derive(Debug, thiserror::Error)]
pub enum PathsError {
    /// The repository root does not exist or cannot be canonicalized.
    #[error(
        "cannot use {path} as the repository root: {source}; pass the existing top-level directory of the working tree"
    )]
    Root {
        /// The path that was given.
        path: PathBuf,
        /// The underlying error.
        source: io::Error,
    },
    /// The repository root exists but is not a directory.
    #[error("cannot use {path} as the repository root: it is not a directory")]
    NotADirectory {
        /// The path that was given.
        path: PathBuf,
    },
}

/// One component of a path still to be walked.
#[derive(Debug, Clone)]
pub(crate) enum Comp {
    Cur,
    Parent,
    Name(OsString),
}

/// Why a walk stopped without a path.
enum Stop {
    Unresolvable(String),
    Device(String),
}

/// The outcome of a successful walk.
pub(crate) struct Walked {
    /// Absolute, link-free path (the part that exists is canonical; the rest is lexical).
    pub path: PathBuf,
    pub via_link: bool,
}

/// Normalizes paths for one repository and classifies where they land.
///
/// Build one per hook invocation with [`PathResolver::new`]; it probes the filesystem once.
#[derive(Debug, Clone)]
pub struct PathResolver {
    root: PathBuf,
    root_base: PathBuf,
    root_names: Vec<OsString>,
    git_dir: Option<PathBuf>,
    /// Git directories that live outside the root (linked worktrees, submodules); paths in them
    /// are reported as `.git/...`.
    git_aliases: Vec<PathBuf>,
    case_insensitive: bool,
}

impl PathResolver {
    /// Build a resolver for the working tree rooted at `repo_root`.
    ///
    /// Canonicalizes the root, finds the worktree's git directory (following a `.git` file for
    /// linked worktrees and submodules) and probes whether the filesystem is case-insensitive.
    ///
    /// # Errors
    ///
    /// Fails when the root does not exist or is not a directory.
    pub fn new(repo_root: &Path) -> Result<Self, PathsError> {
        let root = canonical(repo_root).map_err(|source| PathsError::Root {
            path: repo_root.to_path_buf(),
            source,
        })?;
        if !root.is_dir() {
            return Err(PathsError::NotADirectory { path: root });
        }
        let (root_base, root_comps) = split(&root);
        let root_names = root_comps
            .into_iter()
            .filter_map(|c| match c {
                Comp::Name(n) => Some(n),
                Comp::Cur | Comp::Parent => None,
            })
            .collect();
        let (git_dir, git_aliases) = find_git_dirs(&root);
        let case_insensitive = probe_case_insensitive(&root);
        Ok(Self {
            root,
            root_base,
            root_names,
            git_dir,
            git_aliases,
            case_insensitive,
        })
    }

    /// Override the detected case sensitivity (for tests and for callers that know better).
    #[must_use]
    pub fn with_case_insensitive(mut self, case_insensitive: bool) -> Self {
        self.case_insensitive = case_insensitive;
        self
    }

    /// The canonical repository root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The canonical git directory of this worktree, if there is one. For a linked worktree or
    /// a submodule this is outside the root.
    #[must_use]
    pub fn git_dir(&self) -> Option<&Path> {
        self.git_dir.as_deref()
    }

    /// Whether the filesystem holding the root is case-insensitive. Compile the policy with
    /// this value so sealed globs match the way paths are spelled on disk.
    #[must_use]
    pub fn case_insensitive(&self) -> bool {
        self.case_insensitive
    }

    /// Resolve `raw` (absolute, or relative to `cwd`) the way a write or read through it would:
    /// every link on the way, including the last component, is followed.
    #[must_use]
    pub fn resolve(&self, raw: &str, cwd: &Path) -> ResolvedPath {
        self.build(raw, Path::new(raw), cwd, true)
    }

    /// Resolve `raw` the way `rm` and `mv` see it: a link in the last component is the thing
    /// acted on and is not followed, unless `raw` ends with a separator. Links in the leading
    /// components are followed as usual.
    #[must_use]
    pub fn resolve_entry(&self, raw: &str, cwd: &Path) -> ResolvedPath {
        self.build(raw, Path::new(raw), cwd, has_trailing_separator(raw))
    }

    pub(crate) fn build(
        &self,
        raw: &str,
        path: &Path,
        cwd: &Path,
        follow_last: bool,
    ) -> ResolvedPath {
        let (location, via_link) = match self.walk(path, cwd, follow_last) {
            Ok(w) => (self.classify(&w.path), w.via_link),
            Err(Stop::Unresolvable(reason)) => (PathLocation::Unresolvable(reason), false),
            Err(Stop::Device(name)) => (PathLocation::Outside(name), false),
        };
        ResolvedPath {
            raw: raw.to_owned(),
            location,
            via_link,
            linked_into_repo: Vec::new(),
        }
    }

    /// The filesystem path a [`PathLocation`] stands for, if it has one.
    pub(crate) fn fs_path(&self, location: &PathLocation) -> Option<PathBuf> {
        match location {
            PathLocation::InRepo(rel) => Some(self.fs_path_of_rel(rel.as_str())),
            PathLocation::Outside(abs) => Some(PathBuf::from(abs)),
            PathLocation::Unresolvable(_) => None,
        }
    }

    /// The filesystem path of a repository-relative path (`.` is the root).
    pub(crate) fn fs_path_of_rel(&self, rel: &str) -> PathBuf {
        if rel == "." {
            return self.root.clone();
        }
        if let Some(rest) = rel.strip_prefix(".git") {
            let inside = rest.is_empty() || rest.starts_with('/');
            if inside && !self.git_aliases.is_empty() {
                let rest = rest.trim_start_matches('/');
                for alias in &self.git_aliases {
                    let candidate = join_rel(alias, rest);
                    if candidate.exists() {
                        return candidate;
                    }
                }
                return join_rel(&self.git_aliases[0], rest);
            }
        }
        join_rel(&self.root, rel)
    }

    /// The components of `path` below `prefix`, when it starts with it (ignoring case on
    /// case-insensitive filesystems). Both must be absolute and normalized.
    pub(crate) fn strip<'a>(&self, path: &'a Path, prefix: &Path) -> Option<Vec<&'a OsStr>> {
        let mut own = path.components();
        for want in prefix.components() {
            let have = own.next()?;
            if !names_eq(have.as_os_str(), want.as_os_str(), self.case_insensitive) {
                return None;
            }
        }
        Some(own.map(Component::as_os_str).collect())
    }

    /// Classify an absolute, link-free path.
    pub(crate) fn classify(&self, path: &Path) -> PathLocation {
        let repo = |prefix: Option<&str>, rest: &[&OsStr]| {
            let mut parts: Vec<String> = prefix.map(str::to_owned).into_iter().collect();
            parts.extend(rest.iter().map(|n| n.to_string_lossy().into_owned()));
            let joined = if parts.is_empty() {
                ".".to_owned()
            } else {
                parts.join("/")
            };
            match RepoPath::try_from(joined) {
                Ok(p) => PathLocation::InRepo(p),
                Err(e) => PathLocation::Unresolvable(e.to_string()),
            }
        };
        if let Some(rest) = self.strip(path, &self.root) {
            return repo(None, &rest);
        }
        for alias in &self.git_aliases {
            if let Some(rest) = self.strip(path, alias) {
                return repo(Some(".git"), &rest);
            }
        }
        PathLocation::Outside(path.to_string_lossy().into_owned())
    }

    /// Follow `raw` from `cwd`: links in leading components are always followed; the last
    /// component only when `follow_last`.
    fn walk(&self, raw: &Path, cwd: &Path, follow_last: bool) -> Result<Walked, Stop> {
        if raw.as_os_str().is_empty() {
            return Err(Stop::Unresolvable(
                "the path is empty; give a file or directory".into(),
            ));
        }
        if raw.to_string_lossy().contains('\0') {
            return Err(Stop::Unresolvable("the path contains a NUL byte".into()));
        }
        let abs = absolutize(raw, cwd).map_err(Stop::Unresolvable)?;
        let (base, comps) = split(&abs);
        let (mut resolved, mut pending) = self.anchor(base, comps);
        let mut tail: Vec<OsString> = Vec::new();
        let (mut hops, mut via_link) = (0, false);

        while let Some(comp) = pending.pop_front() {
            let mut name = match comp {
                Comp::Cur => continue,
                Comp::Parent => {
                    if tail.pop().is_none() {
                        resolved.pop();
                    }
                    continue;
                }
                Comp::Name(name) => name,
            };
            if cfg!(windows) {
                let text = name.to_string_lossy();
                if is_windows_device(&text) {
                    return Err(Stop::Device(format!(r"\\.\{}", text.to_ascii_uppercase())));
                }
                if let std::borrow::Cow::Owned(trimmed) = windows_name(&text) {
                    name = trimmed.into();
                }
            }
            if !tail.is_empty() {
                tail.push(name);
                continue;
            }
            if self.case_insensitive
                && !cfg!(windows)
                && let Some(found) = on_disk_name(&resolved, &name)
            {
                name = found;
            }
            let candidate = resolved.join(&name);
            match fs::symlink_metadata(&candidate) {
                Ok(meta)
                    if meta.file_type().is_symlink() && (follow_last || !pending.is_empty()) =>
                {
                    hops += 1;
                    if hops > MAX_LINK_HOPS {
                        return Err(Stop::Unresolvable(format!(
                            "too many levels of links while resolving {} (a link loop?)",
                            raw.display()
                        )));
                    }
                    let target = fs::read_link(&candidate).map_err(|e| {
                        Stop::Unresolvable(format!(
                            "cannot read the link {}: {e}",
                            candidate.display()
                        ))
                    })?;
                    via_link |= self.counts_as_link(&candidate);
                    let (target_base, target_comps) = split(&simplify_link_target(&target));
                    if !target_base.as_os_str().is_empty() {
                        resolved = anchor_rooted(target_base, &resolved);
                    }
                    for comp in target_comps.into_iter().rev() {
                        pending.push_front(comp);
                    }
                }
                Ok(_) => resolved = candidate,
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                    ) =>
                {
                    tail.push(name);
                }
                Err(e) => {
                    return Err(Stop::Unresolvable(format!(
                        "cannot read {}: {e}",
                        candidate.display()
                    )));
                }
            }
        }

        // Windows: let the OS fix drive and name case (and expand 8.3 names) of what exists.
        // `canonicalize` follows a link in the last component, so when that link must not be
        // followed, canonicalize its parent and keep the link's own name.
        if cfg!(windows) && resolved != self.root {
            let keep_last_link = !follow_last
                && tail.is_empty()
                && fs::symlink_metadata(&resolved).is_ok_and(|m| m.file_type().is_symlink());
            let fixed = if keep_last_link {
                match (resolved.parent(), resolved.file_name()) {
                    (Some(parent), Some(name)) => canonical(parent).ok().map(|p| p.join(name)),
                    _ => None,
                }
            } else {
                canonical(&resolved).ok()
            };
            if let Some(fixed) = fixed {
                resolved = fixed;
            }
        }
        resolved.extend(tail);
        Ok(Walked {
            path: resolved,
            via_link,
        })
    }

    /// Start the walk: below the root when the path is spelled under it (the root is already
    /// canonical, so its components need no lookups), else from the filesystem root.
    fn anchor(&self, base: PathBuf, comps: Vec<Comp>) -> (PathBuf, VecDeque<Comp>) {
        let ci = self.case_insensitive;
        let n = self.root_names.len();
        let under_root = names_eq(base.as_os_str(), self.root_base.as_os_str(), ci)
            && comps.len() >= n
            && comps
                .iter()
                .zip(&self.root_names)
                .all(|(comp, want)| matches!(comp, Comp::Name(have) if names_eq(have, want, ci)));
        if under_root {
            (self.root.clone(), comps.into_iter().skip(n).collect())
        } else {
            (base, comps.into())
        }
    }

    /// Whether a link on the way should make the result `via_link`. A link that only names an
    /// ancestor of the root (`/var` -> `/private/var`, a symlinked home) is part of where the
    /// repository lives, not a way into or out of it.
    fn counts_as_link(&self, link: &Path) -> bool {
        if self.strip(link, &self.root).is_some() {
            return true;
        }
        match canonical(link) {
            Ok(target) => !(target != self.root && self.strip(&self.root, &target).is_some()),
            Err(_) => true,
        }
    }
}

fn has_trailing_separator(raw: &str) -> bool {
    raw.ends_with('/') || (cfg!(windows) && raw.ends_with('\\'))
}

/// Join a `/`-separated relative path onto `base`.
pub(crate) fn join_rel(base: &Path, rel: &str) -> PathBuf {
    let mut out = base.to_path_buf();
    out.extend(rel.split('/').filter(|s| !s.is_empty() && *s != "."));
    out
}

/// Split a path into its prefix and root (`/`, `C:\`, `\\server\share\`) and the components
/// after them.
pub(crate) fn split(path: &Path) -> (PathBuf, Vec<Comp>) {
    let mut base = PathBuf::new();
    let mut comps = Vec::new();
    for comp in path.components() {
        match comp {
            Component::Prefix(_) | Component::RootDir => base.push(comp.as_os_str()),
            Component::CurDir => comps.push(Comp::Cur),
            Component::ParentDir => comps.push(Comp::Parent),
            Component::Normal(name) => comps.push(Comp::Name(name.to_owned())),
        }
    }
    (base, comps)
}

/// Make `raw` absolute against `cwd` without touching the filesystem.
pub(crate) fn absolutize(raw: &Path, cwd: &Path) -> Result<PathBuf, String> {
    let raw = unverbatim(raw);
    if raw.is_absolute() {
        return Ok(raw);
    }
    let cwd = unverbatim(cwd);
    if !cwd.is_absolute() {
        return Err(format!(
            "the working directory {} is not absolute; the event must carry an absolute cwd",
            cwd.display()
        ));
    }
    #[cfg(windows)]
    {
        if let Some(Component::Prefix(drive)) = raw.components().next() {
            // `C:name` is relative to the current directory of drive C.
            return match cwd.components().next() {
                Some(Component::Prefix(have))
                    if names_eq(have.as_os_str(), drive.as_os_str(), true) =>
                {
                    Ok(cwd.join(raw.components().skip(1).collect::<PathBuf>()))
                }
                _ => Err(format!(
                    "{} is relative to the current directory of another drive; use a full path",
                    raw.display()
                )),
            };
        }
    }
    // On Windows `\name` keeps the drive of `cwd`; `join` does exactly that.
    Ok(cwd.join(&raw))
}

/// Turn a rooted link target without a drive (`\name` on Windows) into one on the drive of the
/// directory holding the link; leave every other base alone.
fn anchor_rooted(target_base: PathBuf, link_dir: &Path) -> PathBuf {
    if cfg!(windows)
        && target_base.has_root()
        && let Some(prefix @ Component::Prefix(_)) = link_dir.components().next()
        && !matches!(target_base.components().next(), Some(Component::Prefix(_)))
    {
        return Path::new(prefix.as_os_str()).join(target_base);
    }
    target_base
}

/// Strip the NT namespace prefix some junction targets carry, and the `\\?\` form when it is
/// not needed.
fn simplify_link_target(target: &Path) -> PathBuf {
    let text = target.to_string_lossy();
    let text = text.strip_prefix(r"\??\").unwrap_or(&text);
    unverbatim(Path::new(text))
}

/// Find the worktree's git directory and the ones outside the root that must be reported as
/// `.git/...`.
fn find_git_dirs(root: &Path) -> (Option<PathBuf>, Vec<PathBuf>) {
    let dotgit = root.join(".git");
    let outside = |dirs: Vec<PathBuf>| {
        dirs.into_iter()
            .filter(|d| !d.starts_with(root))
            .collect::<Vec<_>>()
    };
    let Ok(meta) = fs::metadata(&dotgit) else {
        return (None, Vec::new());
    };
    if meta.is_dir() {
        let canonical = canonical(&dotgit).ok();
        return (canonical.clone(), outside(canonical.into_iter().collect()));
    }
    // A linked worktree or submodule: `.git` is a file reading `gitdir: <path>`.
    let Ok(text) = fs::read_to_string(&dotgit) else {
        return (None, Vec::new());
    };
    let Some(line) = text.lines().find_map(|l| l.strip_prefix("gitdir:")) else {
        return (None, Vec::new());
    };
    let Ok(git_dir) = canonical(root.join(line.trim())) else {
        return (None, Vec::new());
    };
    let mut dirs = vec![git_dir.clone()];
    if let Ok(common) = fs::read_to_string(git_dir.join("commondir"))
        && let Ok(common) = canonical(git_dir.join(common.trim()))
    {
        dirs.push(common);
    }
    (Some(git_dir), outside(dirs))
}

/// Whether the filesystem holding `root` ignores case: look up the root under a name with its
/// letter case swapped and see whether it is the same directory.
fn probe_case_insensitive(root: &Path) -> bool {
    for dir in root.ancestors() {
        let Some(name) = dir.file_name() else { break };
        let Some(flipped) = flip_case(name) else {
            continue;
        };
        let other = dir.with_file_name(flipped);
        return fs::symlink_metadata(&other).is_ok() && same_entry(dir, &other);
    }
    false
}

#[cfg(unix)]
fn same_entry(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (fs::metadata(a), fs::metadata(b)) {
        (Ok(x), Ok(y)) => x.dev() == y.dev() && x.ino() == y.ino(),
        _ => false,
    }
}

#[cfg(not(unix))]
fn same_entry(a: &Path, b: &Path) -> bool {
    // `canonicalize` reports the on-disk case on Windows, so equal results are one entry.
    matches!((canonical(a), canonical(b)), (Ok(x), Ok(y)) if x == y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nt_namespace_prefixes_are_stripped_from_link_targets() {
        assert_eq!(
            simplify_link_target(Path::new(r"\??\C:\work\repo")),
            PathBuf::from(r"C:\work\repo")
        );
        assert_eq!(
            simplify_link_target(Path::new("plain/rel")),
            PathBuf::from("plain/rel")
        );
    }
}
