//! What a recursive operation reaches: the links inside a directory subtree that lead into the
//! repository, and the repository files under the subtree and under those links.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use acs_core::RepoPath;
use acs_core::gate::{Expansion, PathLocation, ResolvedPath};

use crate::names::canonical;
use crate::resolver::PathResolver;

/// A [`ResolvedPath`] for a directory a recursive operation would enter, plus whether the walk
/// that found its links was cut short.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecursiveResolution {
    /// The path, with `linked_into_repo` filled in.
    pub path: ResolvedPath,
    /// The walk hit its cap (or could not read a directory), so links may have been missed.
    pub truncated: bool,
}

/// Accumulates what a set of walks finds.
pub(crate) struct Reach {
    /// Repository-relative files found (links count as one entry each).
    pub files: BTreeSet<String>,
    /// Repository-relative targets of links that lead into the repository from outside the walked
    /// subtree.
    pub links: BTreeSet<String>,
    pub truncated: bool,
    entries: usize,
    max_entries: usize,
    /// `None` when only links are wanted and files need not be collected.
    max_files: Option<usize>,
}

impl Reach {
    pub(crate) fn new(max_entries: usize, max_files: Option<usize>) -> Self {
        Self {
            files: BTreeSet::new(),
            links: BTreeSet::new(),
            truncated: false,
            entries: 0,
            max_entries,
            max_files,
        }
    }

    pub(crate) fn add_file(&mut self, rel: String) {
        let Some(max) = self.max_files else { return };
        if self.files.contains(&rel) {
            return;
        }
        if self.files.len() >= max {
            self.truncated = true;
        } else {
            self.files.insert(rel);
        }
    }
}

impl PathResolver {
    /// Resolve `raw` as the directory of a recursive operation (`rm -r`, `find -delete`,
    /// `cp -r`) and list, in [`ResolvedPath::linked_into_repo`], the repository paths reachable
    /// through symlinks and junctions inside that subtree.
    ///
    /// The walk does not follow links (so it cannot loop); for each link it resolves the target
    /// and records it when it lands in the repository, outside the subtree being walked. `cap`
    /// bounds the number of directory entries examined; use [`Self::resolve_recursive_report`]
    /// to learn whether it was hit.
    #[must_use]
    pub fn resolve_recursive(&self, raw: &str, cwd: &Path, cap: usize) -> ResolvedPath {
        self.resolve_recursive_report(raw, cwd, cap).path
    }

    /// [`Self::resolve_recursive`], also reporting whether the walk was truncated.
    #[must_use]
    pub fn resolve_recursive_report(
        &self,
        raw: &str,
        cwd: &Path,
        cap: usize,
    ) -> RecursiveResolution {
        self.recursive_from(self.resolve(raw, cwd), cap)
    }

    /// Fill in `linked_into_repo` of an already resolved directory.
    pub(crate) fn recursive_from(&self, mut path: ResolvedPath, cap: usize) -> RecursiveResolution {
        let mut reach = Reach::new(cap, None);
        match &path.location {
            PathLocation::Unresolvable(_) => reach.truncated = true,
            location => {
                if let Some(start) = self.fs_path(location) {
                    let rel = path.repo_path().map(RepoPath::as_str);
                    self.walk_entry(&start, rel, &mut reach);
                }
            }
        }
        path.linked_into_repo = reach
            .links
            .iter()
            .filter_map(|l| RepoPath::try_from(l.clone()).ok())
            .collect();
        RecursiveResolution {
            path,
            truncated: reach.truncated,
        }
    }

    /// List the repository files a recursive operation on `target` reaches: the files under it
    /// when it is in the repository (a link counts as one entry and is not followed), plus the
    /// files under every repository path in `target.linked_into_repo` and under links found
    /// inside the subtree (followed one hop, because a recursive delete through a junction
    /// reaches them).
    ///
    /// For a directory outside the repository, pass the result of [`Self::resolve_recursive`]:
    /// only its `linked_into_repo` entries are listed. At most `cap` files are returned; beyond
    /// that `truncated` is set. The files are sorted.
    #[must_use]
    pub fn expand_recursive(&self, target: &ResolvedPath, cap: usize) -> Expansion {
        let mut reach = Reach::new(cap.saturating_mul(4).saturating_add(256), Some(cap));
        let mut hops: BTreeSet<String> = target
            .linked_into_repo
            .iter()
            .map(|p| p.as_str().to_owned())
            .collect();
        match &target.location {
            PathLocation::InRepo(rel) => {
                self.walk_entry(
                    &self.fs_path_of_rel(rel.as_str()),
                    Some(rel.as_str()),
                    &mut reach,
                );
                hops.append(&mut reach.links);
            }
            PathLocation::Outside(_) => {}
            PathLocation::Unresolvable(_) => reach.truncated = true,
        }
        for rel in collapse(&hops) {
            self.walk_entry(&self.fs_path_of_rel(&rel), Some(&rel), &mut reach);
        }
        into_expansion(reach)
    }

    /// Walk `start` without following links: a directory is walked, anything else is one entry.
    /// `rel` is the repository path of `start` when it is inside the repository.
    pub(crate) fn walk_entry(&self, start: &Path, rel: Option<&str>, reach: &mut Reach) {
        let Ok(meta) = fs::symlink_metadata(start) else {
            return;
        };
        if meta.is_dir() {
            self.walk_tree(start, rel, reach);
        } else {
            if meta.file_type().is_symlink() {
                self.note_link(start, rel, reach);
            }
            if let Some(rel) = rel {
                reach.add_file(rel.to_owned());
            }
        }
    }

    fn walk_tree(&self, start: &Path, start_rel: Option<&str>, reach: &mut Reach) {
        let mut stack = vec![(start.to_path_buf(), start_rel.map(str::to_owned))];
        while let Some((dir, dir_rel)) = stack.pop() {
            let Ok(entries) = fs::read_dir(&dir) else {
                reach.truncated = true;
                continue;
            };
            for entry in entries {
                let Ok(entry) = entry else {
                    reach.truncated = true;
                    continue;
                };
                reach.entries += 1;
                if reach.entries > reach.max_entries {
                    reach.truncated = true;
                }
                if reach.truncated {
                    return;
                }
                let Ok(kind) = entry.file_type() else {
                    reach.truncated = true;
                    continue;
                };
                let path = entry.path();
                let rel = dir_rel.as_deref().map(|d| {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if d == "." {
                        name
                    } else {
                        format!("{d}/{name}")
                    }
                });
                if kind.is_symlink() {
                    self.note_link(&path, start_rel, reach);
                    if let Some(rel) = rel {
                        reach.add_file(rel);
                    }
                } else if kind.is_dir() {
                    stack.push((path, rel));
                } else if let Some(rel) = rel {
                    reach.add_file(rel);
                }
            }
        }
    }

    /// Record where the link at `link` leads when that is inside the repository and not inside
    /// the subtree rooted at `walk_root` (a link back into its own directory adds nothing).
    fn note_link(&self, link: &Path, walk_root: Option<&str>, reach: &mut Reach) {
        let Ok(target) = canonical(link) else {
            return;
        };
        let PathLocation::InRepo(target) = self.classify(&target) else {
            return;
        };
        let target = target.as_str();
        let inside_walk = walk_root.is_some_and(|root| {
            root == "."
                || target == root
                || target
                    .strip_prefix(root)
                    .is_some_and(|rest| rest.starts_with('/'))
        });
        if !inside_walk {
            reach.links.insert(target.to_owned());
        }
    }
}

/// Drop every path that lies under another one in the set (`.` swallows everything).
fn collapse(paths: &BTreeSet<String>) -> Vec<String> {
    paths
        .iter()
        .filter(|p| {
            !paths.iter().any(|q| {
                q != *p
                    && (q.as_str() == "."
                        || p.strip_prefix(q.as_str())
                            .is_some_and(|r| r.starts_with('/')))
            })
        })
        .cloned()
        .collect()
}

/// Turn the files a walk found into an [`Expansion`].
pub(crate) fn into_expansion(reach: Reach) -> Expansion {
    let cap = reach.max_files.unwrap_or(usize::MAX);
    into_expansion_parts(reach.files, reach.truncated, cap)
}

/// Build an [`Expansion`] from repository-relative files, keeping at most `cap` (the first in
/// sorted order) and marking the result truncated when some were dropped.
pub(crate) fn into_expansion_parts(
    files: BTreeSet<String>,
    truncated: bool,
    cap: usize,
) -> Expansion {
    let over = files.len() > cap;
    Expansion {
        files: files
            .into_iter()
            .take(cap)
            .filter_map(|f| RepoPath::try_from(f).ok())
            .collect(),
        truncated: truncated || over,
    }
}
