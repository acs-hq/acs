//! Expanding a shell glob pattern against the working tree.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use acs_core::gate::{Expansion, PathLocation};
use globset::{GlobBuilder, GlobMatcher};

use crate::reach::into_expansion_parts;
use crate::resolver::{Comp, PathResolver, absolutize, split};

/// Whether `text` contains a glob metacharacter (`*`, `?` or `[`).
#[must_use]
pub fn has_glob_meta(text: &str) -> bool {
    text.contains(['*', '?', '['])
}

impl PathResolver {
    /// List the repository files a shell glob (or a plain path) reaches.
    ///
    /// Semantics follow the shell: `*`, `?` and `[...]` match within one path segment, `*` and
    /// `?` do not match a leading `.`, and `**` as a whole segment matches any number of
    /// directories (and, last, everything beneath). `.gitignore` is not consulted: `rm` deletes
    /// ignored files too. Matching ignores case on case-insensitive filesystems.
    ///
    /// With `recursive`, a matched directory contributes every file under it (as
    /// [`Self::expand_recursive`] does, including files reached through links in it);
    /// without, directories are skipped. A matched link is one entry. Walks never follow links
    /// except as `expand_recursive` describes. At most `cap` files are returned, sorted;
    /// `truncated` is set when the pattern reached more, or the scan budget (a multiple of
    /// `cap`) ran out.
    #[must_use]
    pub fn expand_glob(&self, pattern: &str, cwd: &Path, recursive: bool, cap: usize) -> Expansion {
        let mut files = BTreeSet::new();
        let mut truncated = false;
        let unreadable = Expansion {
            files: Vec::new(),
            truncated: true,
        };
        let pat = dunce::simplified(Path::new(pattern));
        let (start, comps) = if pat.has_root() || pat.components().next().is_some_and(is_prefix) {
            match absolutize(pat, cwd) {
                Ok(abs) => {
                    let (base, comps) = split(&abs);
                    (base, comps)
                }
                Err(_) => return unreadable,
            }
        } else if cwd.is_absolute() {
            (cwd.to_path_buf(), split(pat).1)
        } else {
            return unreadable;
        };
        let dirs_only = pattern.ends_with('/') || (cfg!(windows) && pattern.ends_with('\\'));
        let mut budget = cap.saturating_mul(16).saturating_add(1024);

        let mut candidates = vec![start];
        let last_index = comps.len().saturating_sub(1);
        for (i, comp) in comps.into_iter().enumerate() {
            let name = match comp {
                Comp::Cur => continue,
                Comp::Parent => {
                    candidates = candidates.into_iter().map(|c| c.join("..")).collect();
                    continue;
                }
                Comp::Name(name) => name,
            };
            let text = name.to_string_lossy();
            let matcher = if has_glob_meta(&text) && text != "**" {
                self.component_matcher(&text)
            } else {
                None
            };
            let mut next = Vec::new();
            if text == "**" {
                for dir in &candidates {
                    descend(dir, i == last_index, &mut next, &mut budget, &mut truncated);
                }
            } else if let Some(matcher) = matcher {
                let hidden_ok = text.starts_with('.');
                for dir in &candidates {
                    let Ok(entries) = fs::read_dir(dir) else {
                        continue;
                    };
                    for entry in entries.flatten() {
                        if budget == 0 {
                            truncated = true;
                            break;
                        }
                        budget -= 1;
                        let found = entry.file_name();
                        let found_text = found.to_string_lossy();
                        if (hidden_ok || !found_text.starts_with('.'))
                            && matcher.is_match(&*found_text)
                        {
                            next.push(dir.join(&found));
                        }
                    }
                }
            } else {
                next = candidates.into_iter().map(|c| c.join(&name)).collect();
            }
            candidates = next;
        }

        for matched in candidates {
            let Ok(meta) = fs::symlink_metadata(&matched) else {
                continue;
            };
            let is_link = meta.file_type().is_symlink();
            if dirs_only && !matched.is_dir() {
                continue;
            }
            let raw = matched.to_string_lossy().into_owned();
            let entry = self.build(&raw, &matched, cwd, dirs_only);
            if is_link && !dirs_only {
                if let PathLocation::InRepo(rel) = &entry.location {
                    files.insert(rel.as_str().to_owned());
                }
            }
            let left = cap.saturating_sub(files.len());
            if recursive {
                let followed = self.build(&raw, &matched, cwd, true);
                let walked = self.recursive_from(followed, budget.max(cap));
                let expansion = self.expand_recursive(&walked.path, left.max(1));
                truncated |= walked.truncated | expansion.truncated;
                files.extend(expansion.files.into_iter().map(|f| f.as_str().to_owned()));
            } else if meta.is_file()
                && let PathLocation::InRepo(rel) = &entry.location
            {
                files.insert(rel.as_str().to_owned());
            }
            if files.len() > cap {
                break;
            }
        }
        into_expansion_parts(files, truncated, cap)
    }

    /// A matcher for one pattern segment, or `None` when it is not a valid glob (the shell
    /// treats such a segment literally).
    fn component_matcher(&self, text: &str) -> Option<GlobMatcher> {
        GlobBuilder::new(text)
            .literal_separator(true)
            .backslash_escape(!cfg!(windows))
            .case_insensitive(self.case_insensitive())
            .build()
            .ok()
            .map(|glob| glob.compile_matcher())
    }
}

fn is_prefix(comp: std::path::Component<'_>) -> bool {
    matches!(comp, std::path::Component::Prefix(_))
}

/// Push `dir` and every directory under it (not following links); with `files`, the other
/// entries too.
fn descend(
    dir: &Path,
    files: bool,
    out: &mut Vec<PathBuf>,
    budget: &mut usize,
    truncated: &mut bool,
) {
    out.push(dir.to_path_buf());
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            if *budget == 0 {
                *truncated = true;
                return;
            }
            *budget -= 1;
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                out.push(entry.path());
                stack.push(entry.path());
            } else if files {
                out.push(entry.path());
            }
        }
    }
}
