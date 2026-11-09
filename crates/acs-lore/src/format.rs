//! Lore files: parsing one, and loading every one under `.acs/policy/lore/`.
//!
//! A lore file is a Markdown file with TOML front matter between two `+++` lines
//! ([`acs_core::split_front_matter`]). Loading never fails: a file that cannot be read or parsed
//! becomes a [`LoreProblem`] and is left out, so one broken entry cannot take the others with it.

use std::fmt;
use std::path::Path;

use acs_core::{LoreFrontMatter, LoreId, split_front_matter};
use acs_policy::glob::{Glob, Specificity};

/// Where lore lives, relative to the work tree root.
pub const LORE_DIR: &str = ".acs/policy/lore";

/// The largest lore file read, in bytes. Lore is injected into agents' context, so anything
/// near this is a mistake.
pub const MAX_FILE_BYTES: u64 = 256 * 1024;

/// How deep the lore directory is walked.
const MAX_DEPTH: usize = 32;

/// One lore entry: its front matter, its body and where it came from.
#[derive(Debug, Clone)]
pub struct LoreEntry {
    /// The front matter.
    pub front: LoreFrontMatter,
    /// The body, without leading or trailing blank space.
    pub body: String,
    /// The file, relative to the work tree root and `/`-separated
    /// (`.acs/policy/lore/payments/money.md`).
    pub file: String,
    globs: Vec<Glob>,
}

impl LoreEntry {
    /// Parses the lore file `file` (repository-relative, for messages) with content `text`.
    ///
    /// # Errors
    ///
    /// A [`LoreProblem`] when the fences, the TOML, a scope glob or the body are not right.
    pub fn parse(file: &str, text: &str, case_insensitive: bool) -> Result<Self, LoreProblem> {
        let problem = |message: String| LoreProblem {
            file: file.to_owned(),
            message,
        };
        let (front, body) = split_front_matter(text).map_err(|err| problem(err.to_string()))?;
        let front: LoreFrontMatter = toml::from_str(front).map_err(|err| {
            problem(format!(
                "the front matter is not valid: {}",
                err.message().trim_end()
            ))
        })?;
        if front.scope.is_empty() {
            return Err(problem(
                "`scope` is empty: list the paths the entry applies to, such as \
                 [\"payments/**\"]"
                    .to_owned(),
            ));
        }
        let globs = front
            .scope
            .iter()
            .map(|pattern| {
                Glob::new(pattern, case_insensitive).map_err(|err| {
                    problem(format!(
                        "scope `{pattern}` is not a valid glob: {err} ({})",
                        err.fix
                    ))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let body = body.trim();
        if body.is_empty() {
            return Err(problem(
                "the entry has no text after its front matter".to_owned(),
            ));
        }
        Ok(Self {
            front,
            body: body.to_owned(),
            file: file.to_owned(),
            globs,
        })
    }

    /// The entry's id.
    #[must_use]
    pub fn id(&self) -> &LoreId {
        &self.front.id
    }

    /// How specifically the entry's scope covers `path` (repository-relative), or `None` when
    /// it does not cover it.
    #[must_use]
    pub fn match_specificity(&self, path: &str) -> Option<Specificity> {
        self.globs
            .iter()
            .filter_map(|glob| glob.match_specificity(path))
            .max()
    }

    /// Whether the entry's scope covers `path` (repository-relative).
    #[must_use]
    pub fn covers(&self, path: &str) -> bool {
        self.globs.iter().any(|glob| glob.is_match(path))
    }
}

/// Renders a lore file: the front matter between `+++` lines, a blank line, then the body.
///
/// # Errors
///
/// Only if the front matter cannot be serialized, which its types make impossible in practice.
pub fn render_file(front: &LoreFrontMatter, body: &str) -> Result<String, String> {
    let toml = toml::to_string(front).map_err(|err| err.to_string())?;
    Ok(format!(
        "+++\n{}\n+++\n\n{}\n",
        toml.trim_end(),
        body.trim()
    ))
}

/// A lore file that was left out, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoreProblem {
    /// The file, relative to the work tree root.
    pub file: String,
    /// What is wrong with it.
    pub message: String,
}

impl fmt::Display for LoreProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.file, self.message)
    }
}

/// Every lore entry of a work tree.
#[derive(Debug, Clone, Default)]
pub struct LoreSet {
    entries: Vec<LoreEntry>,
    problems: Vec<LoreProblem>,
}

impl LoreSet {
    /// Loads every `*.md` file under `<work_tree>/.acs/policy/lore/`, in path order. Files
    /// named `README*` are documentation and skipped; symbolic links are not followed. A file
    /// whose id was already taken by an earlier file is a problem, not an entry. An absent
    /// directory is an empty set.
    #[must_use]
    pub fn load(work_tree: &Path, case_insensitive: bool) -> Self {
        let root = work_tree.join(LORE_DIR);
        let mut files = Vec::new();
        let mut problems = Vec::new();
        walk(&root, LORE_DIR, 0, &mut files, &mut problems);
        files.sort();
        let mut set = Self {
            entries: Vec::new(),
            problems,
        };
        for rel in files {
            let path = work_tree.join(&rel);
            let read = std::fs::metadata(&path).and_then(|meta| {
                if meta.len() > MAX_FILE_BYTES {
                    Err(std::io::Error::other(format!(
                        "the file is {} KiB; lore files may be at most {} KiB",
                        meta.len() / 1024,
                        MAX_FILE_BYTES / 1024
                    )))
                } else {
                    std::fs::read(&path)
                }
            });
            let text = match read.map(String::from_utf8) {
                Ok(Ok(text)) => text,
                Ok(Err(_)) => {
                    set.problems.push(LoreProblem {
                        file: rel,
                        message: "the file is not valid UTF-8".to_owned(),
                    });
                    continue;
                }
                Err(err) => {
                    set.problems.push(LoreProblem {
                        file: rel,
                        message: format!("cannot read it: {err}"),
                    });
                    continue;
                }
            };
            set.push(LoreEntry::parse(&rel, &text, case_insensitive));
        }
        set
    }

    /// A set from entries already parsed (tests, and callers that parse themselves). Duplicate
    /// ids become problems, as in [`LoreSet::load`].
    #[must_use]
    pub fn from_parsed(parsed: Vec<Result<LoreEntry, LoreProblem>>) -> Self {
        let mut set = Self::default();
        for item in parsed {
            set.push(item);
        }
        set
    }

    fn push(&mut self, item: Result<LoreEntry, LoreProblem>) {
        match item {
            Ok(entry) => {
                if let Some(first) = self.get(entry.id()) {
                    let problem = LoreProblem {
                        message: format!(
                            "id `{}` is already used by {}; give this entry an id of its own",
                            entry.id(),
                            first.file
                        ),
                        file: entry.file,
                    };
                    self.problems.push(problem);
                } else {
                    self.entries.push(entry);
                }
            }
            Err(problem) => self.problems.push(problem),
        }
    }

    /// The valid entries, in file path order.
    #[must_use]
    pub fn entries(&self) -> &[LoreEntry] {
        &self.entries
    }

    /// The files left out, and why.
    #[must_use]
    pub fn problems(&self) -> &[LoreProblem] {
        &self.problems
    }

    /// The entry with id `id`.
    #[must_use]
    pub fn get(&self, id: &LoreId) -> Option<&LoreEntry> {
        self.entries.iter().find(|entry| entry.id() == id)
    }

    /// Whether there are no entries and no problems.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty() && self.problems.is_empty()
    }
}

/// Collects the repository-relative paths of the `*.md` files under `dir`.
fn walk(
    dir: &Path,
    rel: &str,
    depth: usize,
    files: &mut Vec<String>,
    problems: &mut Vec<LoreProblem>,
) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    if depth > MAX_DEPTH {
        problems.push(LoreProblem {
            file: rel.to_owned(),
            message: format!("nested more than {MAX_DEPTH} levels deep; not read"),
        });
        return;
    }
    for item in read.flatten() {
        let Ok(kind) = item.file_type() else { continue };
        let name = item.file_name();
        let Some(name) = name.to_str() else {
            problems.push(LoreProblem {
                file: format!("{rel}/{}", name.to_string_lossy()),
                message: "the name is not valid Unicode; rename it".to_owned(),
            });
            continue;
        };
        let child = format!("{rel}/{name}");
        if kind.is_dir() {
            walk(&item.path(), &child, depth + 1, files, problems);
        } else if kind.is_file()
            && Path::new(name)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
            && !name.to_ascii_lowercase().starts_with("readme")
        {
            files.push(child);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) const MONEY: &str = "+++\nid = \"lore_7f3a\"\nscope = [\"payments/**\"]\n\
        owner = \"sara@example.com\"\nwritten_against = \"a1b2c3d\"\n+++\n\n\
        Amounts are integer cents.\n";

    fn write(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn parses_the_spec_example() {
        let entry = LoreEntry::parse(".acs/policy/lore/payments/money.md", MONEY, false).unwrap();
        assert_eq!(entry.id().as_str(), "lore_7f3a");
        assert_eq!(entry.body, "Amounts are integer cents.");
        assert!(entry.covers("payments/refund.ts"));
        assert!(!entry.covers("lib/money.ts"));
    }

    #[test]
    fn broken_files_are_problems_with_their_reason() {
        let cases = [
            ("no fences", "front matter fence"),
            ("+++\nid = \"lore_x\"\n", "not closed"),
            (
                "+++\nid = \"lore_x\"\nscope = []\nowner = \"a@b\"\nwritten_against = \"x\"\n+++\nbody",
                "`scope` is empty",
            ),
            (
                "+++\nid = \"lore_x\"\nscope = [\"/abs\"]\nowner = \"a@b\"\nwritten_against = \"x\"\n+++\nbody",
                "not a valid glob",
            ),
            (
                "+++\nid = \"lore_x\"\nscope = [\"a/**\"]\nowner = \"a@b\"\nwritten_against = \"x\"\n+++\n\n  \n",
                "no text",
            ),
            (
                "+++\nid = \"lore_x\"\nscope = [\"a/**\"]\nowner = \"nobody\"\nwritten_against = \"x\"\n+++\nbody",
                "invalid owner",
            ),
            (
                "+++\nid = \"lore_x\"\nscope = [\"a/**\"]\nowner = \"a@b\"\nwritten_against = \"x\"\ntypo = 1\n+++\nbody",
                "typo",
            ),
        ];
        for (text, expected) in cases {
            let err = LoreEntry::parse("f.md", text, false).unwrap_err();
            assert!(
                err.message.contains(expected),
                "{text:?}: {} (wanted {expected:?})",
                err.message
            );
        }
    }

    #[test]
    fn load_walks_the_tree_skips_readmes_and_reports_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, ".acs/policy/lore/payments/money.md", MONEY);
        write(root, ".acs/policy/lore/README.md", "How to write lore.");
        write(root, ".acs/policy/lore/notes.txt", "not lore");
        write(
            root,
            ".acs/policy/lore/zz/copy.md",
            &MONEY.replace("cents", "dollars"),
        );
        write(root, ".acs/policy/lore/bad.md", "oops");
        let set = LoreSet::load(root, false);
        assert_eq!(set.entries().len(), 1);
        assert_eq!(set.entries()[0].file, ".acs/policy/lore/payments/money.md");
        let problems: Vec<_> = set.problems().iter().map(|p| p.file.as_str()).collect();
        assert_eq!(
            problems,
            [".acs/policy/lore/bad.md", ".acs/policy/lore/zz/copy.md"]
        );
        assert!(set.problems()[1].message.contains("already used"));
    }

    #[test]
    fn absent_directory_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(LoreSet::load(dir.path(), false).is_empty());
    }

    #[test]
    fn render_file_round_trips() {
        let entry = LoreEntry::parse("f.md", MONEY, false).unwrap();
        let text = render_file(&entry.front, &entry.body).unwrap();
        let again = LoreEntry::parse("f.md", &text, false).unwrap();
        assert_eq!(again.front, entry.front);
        assert_eq!(again.body, entry.body);
    }
}
