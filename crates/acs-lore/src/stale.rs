//! Staleness: how much of an entry's scope changed since the revision it was written against.
//!
//! The share is the lines added plus deleted in the scope's text files between
//! `written_against` and `HEAD`, divided by the lines the scope had at `written_against`. An entry
//! is stale once the share reaches `[lore] stale_percent` (30% by default). Uncommitted changes
//! do not count, so the answer only changes when `HEAD` does, which is what [`StaleCache`]
//! relies on. An entry written against a revision this clone does not have is treated as stale:
//! nothing says it still holds.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use acs_core::Percent;
use acs_git::GitError;
use serde::{Deserialize, Serialize};

use crate::format::LoreEntry;

/// What is known about an entry's staleness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Staleness {
    /// Less than the threshold changed.
    Fresh {
        /// Lines changed as a percentage of the lines the scope had (may exceed 100).
        percent: u32,
    },
    /// At least the threshold changed.
    Stale {
        /// Lines changed as a percentage of the lines the scope had (may exceed 100).
        percent: u32,
        /// The entry's `written_against`.
        since: String,
    },
    /// `written_against` names no commit in this clone.
    UnknownRevision {
        /// The entry's `written_against`.
        since: String,
    },
}

impl Staleness {
    /// Whether the entry should be verified before it is relied on.
    #[must_use]
    pub fn is_stale(&self) -> bool {
        !matches!(self, Self::Fresh { .. })
    }

    /// The line put above a stale entry's text, or `None` for a fresh one.
    #[must_use]
    pub fn verify_note(&self) -> Option<String> {
        match self {
            Self::Fresh { .. } => None,
            Self::Stale { percent, since } => Some(format!(
                "This entry may be out of date: {} of the lines in its scope changed since it was \
                 written (at `{since}`). Check it against the code before relying on it, and \
                 propose a correction with acs_lore_propose if it is wrong.",
                shown_percent(*percent)
            )),
            Self::UnknownRevision { since } => Some(format!(
                "This entry may be out of date: it was written against `{since}`, which this clone \
                 does not have. Check it against the code before relying on it."
            )),
        }
    }

    /// A short description for listings: `12% changed`, `42% changed (stale)`, `unknown
    /// revision a1b2c3d`.
    #[must_use]
    pub fn summary(&self) -> String {
        match self {
            Self::Fresh { percent } => format!("{} changed", shown_percent(*percent)),
            Self::Stale { percent, .. } => format!("{} changed (stale)", shown_percent(*percent)),
            Self::UnknownRevision { since } => format!("unknown revision {since} (stale)"),
        }
    }
}

fn shown_percent(percent: u32) -> String {
    if percent > 100 {
        "over 100%".to_owned()
    } else {
        format!("{percent}%")
    }
}

/// Measures the staleness of `entry` against the commit `head`.
///
/// # Errors
///
/// [`GitError`] when git fails for another reason than an unknown `written_against`.
pub fn measure(
    work_tree: &Path,
    entry: &LoreEntry,
    head: &str,
    threshold: Percent,
    case_insensitive: bool,
) -> Result<Staleness, GitError> {
    let since = entry.front.written_against.clone();
    let Some(since_commit) = acs_git::lore::resolve_commit(work_tree, &since)? else {
        return Ok(Staleness::UnknownRevision { since });
    };
    let pathspecs = pathspecs(&entry.front.scope, case_insensitive);
    let keep = |path: &str| entry.covers(path);
    let change = acs_git::lore::scope_change(work_tree, &since_commit, head, &pathspecs, &keep)?;
    // A scope that had no lines and now has some changed completely.
    let percent = match change
        .lines_changed
        .saturating_mul(100)
        .checked_div(change.lines_then)
    {
        Some(share) => u32::try_from(share).unwrap_or(u32::MAX),
        None if change.lines_changed == 0 => 0,
        None => 100,
    };
    Ok(if percent >= u32::from(threshold.get()) {
        Staleness::Stale { percent, since }
    } else {
        Staleness::Fresh { percent }
    })
}

/// Git pathspecs that cover at least the paths `scope` matches, so git reads no more than it
/// must. Exact membership is decided afterwards with the spec's own glob dialect. Globs with
/// `{a,b}` (which git's glob magic lacks) narrow to their literal leading directories; a glob
/// with no literal start means the whole tree, which is an empty list.
#[must_use]
pub fn pathspecs(scope: &[String], case_insensitive: bool) -> Vec<String> {
    let magic = if case_insensitive {
        ":(glob,icase)"
    } else {
        ":(glob)"
    };
    let mut specs = Vec::new();
    for pattern in scope {
        if pattern.contains('{') {
            let literal: Vec<&str> = pattern
                .split('/')
                .take_while(|seg| !seg.contains(['*', '?', '[', '{']))
                .collect();
            if literal.is_empty() {
                return Vec::new();
            }
            specs.push(format!("{magic}{}/**", literal.join("/")));
        } else {
            specs.push(format!("{magic}{pattern}"));
        }
    }
    specs
}

/// Staleness answers for one `HEAD` and threshold, kept in a JSON file in the repository's
/// cache directory so hooks measure each entry once per commit, not once per session.
#[derive(Debug)]
pub struct StaleCache {
    path: PathBuf,
    file: CacheFile,
    dirty: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct CacheFile {
    head: String,
    threshold: u8,
    entries: BTreeMap<String, Staleness>,
}

/// The cache file's name.
pub const CACHE_FILE: &str = "lore-stale.json";

impl StaleCache {
    /// Opens the cache in `cache_dir` for `head` and `threshold`. A missing or unreadable file,
    /// or one written for another `HEAD` or threshold, starts empty.
    #[must_use]
    pub fn open(cache_dir: &Path, head: &str, threshold: Percent) -> Self {
        let path = cache_dir.join(CACHE_FILE);
        let file = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<CacheFile>(&bytes).ok())
            .filter(|file| file.head == head && file.threshold == threshold.get())
            .unwrap_or_else(|| CacheFile {
                head: head.to_owned(),
                threshold: threshold.get(),
                entries: BTreeMap::new(),
            });
        Self {
            path,
            file,
            dirty: false,
        }
    }

    /// The staleness of `entry`: from the cache, or measured and remembered. `None` when it
    /// could not be measured (git failed); that is not remembered.
    pub fn get_or_measure(
        &mut self,
        work_tree: &Path,
        entry: &LoreEntry,
        case_insensitive: bool,
    ) -> Option<Staleness> {
        let key = key(entry);
        if let Some(known) = self.file.entries.get(&key) {
            return Some(known.clone());
        }
        let threshold = Percent::try_from(self.file.threshold).ok()?;
        let measured = measure(
            work_tree,
            entry,
            &self.file.head,
            threshold,
            case_insensitive,
        )
        .ok()?;
        self.file.entries.insert(key, measured.clone());
        self.dirty = true;
        Some(measured)
    }

    /// Writes the cache back if anything was added. Best effort: a cache that cannot be written
    /// only costs a measurement next time.
    pub fn save(&mut self) {
        if !self.dirty {
            return;
        }
        let Ok(bytes) = serde_json::to_vec(&self.file) else {
            return;
        };
        let tmp = self
            .path
            .with_extension(format!("json.{}.tmp", std::process::id()));
        if std::fs::write(&tmp, bytes).is_ok() && std::fs::rename(&tmp, &self.path).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        self.dirty = false;
    }
}

/// What a cached answer depends on besides `HEAD`: the entry's id, revision and scope.
fn key(entry: &LoreEntry) -> String {
    format!(
        "{}\u{1f}{}\u{1f}{}",
        entry.id(),
        entry.front.written_against,
        entry.front.scope.join("\u{1e}")
    )
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
        assert!(out.status.success(), "git {args:?}");
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    fn write(dir: &Path, rel: &str, text: &str) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn entry(scope: &str, since: &str) -> LoreEntry {
        let text = format!(
            "+++\nid = \"lore_a\"\nscope = [\"{scope}\"]\nowner = \"a@b.c\"\n\
             written_against = \"{since}\"\n+++\nBody.\n"
        );
        LoreEntry::parse("f.md", &text, false).unwrap()
    }

    fn pct(value: u8) -> Percent {
        Percent::try_from(value).unwrap()
    }

    #[test]
    fn measures_against_head_and_the_threshold() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        git(d, &["init", "-q", "-b", "main"]);
        git(d, &["config", "user.email", "a@b.c"]);
        git(d, &["config", "user.name", "A"]);
        let ten: String = (1..=10).map(|n| format!("{n}\n")).collect();
        write(d, "pay/a.rs", &ten);
        write(d, "else/b.rs", &ten);
        git(d, &["add", "."]);
        git(d, &["commit", "-q", "-m", "one"]);
        let since = git(d, &["rev-parse", "--short", "HEAD"]);
        // 2 of 10 lines changed in scope (one line replaced = 1 added + 1 deleted); lots outside.
        write(d, "pay/a.rs", &ten.replace("5\n", "five\n"));
        write(d, "else/b.rs", "all new\n");
        git(d, &["commit", "-q", "-am", "two"]);
        let head = git(d, &["rev-parse", "HEAD"]);

        let e = entry("pay/**", &since);
        assert_eq!(
            measure(d, &e, &head, pct(30), false).unwrap(),
            Staleness::Fresh { percent: 20 }
        );
        assert_eq!(
            measure(d, &e, &head, pct(20), false).unwrap(),
            Staleness::Stale {
                percent: 20,
                since: since.clone()
            }
        );
        let braces = entry("{pay,none}/**", &since);
        assert_eq!(
            measure(d, &braces, &head, pct(30), false).unwrap(),
            Staleness::Fresh { percent: 20 }
        );
        let unknown = entry("pay/**", "0123456789ab");
        assert!(matches!(
            measure(d, &unknown, &head, pct(30), false).unwrap(),
            Staleness::UnknownRevision { .. }
        ));

        // The cache answers without git once measured, and starts over for another HEAD.
        let cache_dir = tempfile::tempdir().unwrap();
        let mut cache = StaleCache::open(cache_dir.path(), &head, pct(30));
        assert_eq!(
            cache.get_or_measure(d, &e, false),
            Some(Staleness::Fresh { percent: 20 })
        );
        cache.save();
        let mut again = StaleCache::open(cache_dir.path(), &head, pct(30));
        assert_eq!(again.file.entries.len(), 1);
        assert!(again.get_or_measure(d, &e, false).is_some());
        let other = StaleCache::open(cache_dir.path(), "other", pct(30));
        assert!(other.file.entries.is_empty());
    }

    #[test]
    fn pathspecs_narrow_without_losing_paths() {
        assert_eq!(
            pathspecs(&["pay/**".to_owned(), "src/{a,b}/*.rs".to_owned()], false),
            [":(glob)pay/**", ":(glob)src/**"]
        );
        assert!(pathspecs(&["{a,b}/**".to_owned()], false).is_empty());
        assert_eq!(pathspecs(&["x".to_owned()], true), [":(glob,icase)x"]);
    }

    #[test]
    fn notes_and_summaries() {
        assert!(Staleness::Fresh { percent: 3 }.verify_note().is_none());
        assert_eq!(
            Staleness::Stale {
                percent: 250,
                since: "abc".into()
            }
            .summary(),
            "over 100% changed (stale)"
        );
    }
}
