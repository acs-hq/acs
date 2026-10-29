//! Reading the policy files, hashing them, and turning them into a [`CompiledPolicy`] with
//! precise errors.

use std::fs;
use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};

use acs_core::{Config, ContentHash, Guards, Territory};
use serde::de::DeserializeOwned;
use toml::Spanned;
use toml::de::{DeTable, DeValue};

use crate::compiled::{At, CompiledPolicy, Found};
use crate::error::{PolicyError, PolicyFile, Problem, ProblemKind};
use crate::files::{PolicyFiles, builtin_guards, resolve_guards};

/// Domain separator of the policy hash; bump it if the hashed framing changes.
const HASH_DOMAIN: &[u8] = b"acs-policy/1\n";

/// The bytes of the three policy files as found on disk (`None` when a file is absent).
#[derive(Debug)]
pub(crate) struct RawFiles {
    dir: PathBuf,
    bytes: [Option<Vec<u8>>; 3],
}

/// Which `guards.toml` keys a file sets explicitly (they replace the built-in defaults).
#[derive(Debug, Clone, Copy, Default)]
struct Presence {
    protected_branches: bool,
    ban: bool,
}

impl RawFiles {
    /// Reads the policy files from `dir`. A missing file (or directory) is absent, not an error.
    pub(crate) fn read(dir: &Path) -> Result<Self, PolicyError> {
        let mut bytes = [None, None, None];
        for (slot, file) in bytes.iter_mut().zip(PolicyFile::ALL) {
            let path = dir.join(file.file_name());
            match fs::read(&path) {
                Ok(content) => *slot = Some(content),
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                Err(source) => return Err(PolicyError::Io { path, source }),
            }
        }
        Ok(Self {
            dir: dir.to_owned(),
            bytes,
        })
    }

    /// The policy hash of these files.
    pub(crate) fn hash(&self) -> ContentHash {
        hash_files(&self.bytes)
    }

    /// Parses, validates and compiles the files.
    pub(crate) fn compile(&self, case_insensitive: bool) -> Result<CompiledPolicy, PolicyError> {
        let mut problems = Vec::new();
        let config = self.parse::<Config>(PolicyFile::Config, &mut problems);
        let guards = self.parse::<Guards>(PolicyFile::Guards, &mut problems);
        let territory = self.parse::<Territory>(PolicyFile::Territory, &mut problems);
        let (Some(config), Some(guards), Some(territory)) = (config, guards, territory) else {
            return Err(PolicyError::Invalid { problems });
        };

        let files = PolicyFiles {
            config: config.map(|(config, _)| config).unwrap_or_default(),
            guards: guards.map_or_else(builtin_guards, |(guards, set)| {
                resolve_guards(guards, set.protected_branches, set.ban)
            }),
            territory: territory
                .map(|(territory, _)| territory)
                .unwrap_or_default(),
        };
        CompiledPolicy::build(files, self.hash(), case_insensitive).map_err(|found| {
            let mut problems: Vec<Problem> = found.into_iter().map(|f| self.locate(f)).collect();
            problems.sort_by_key(|p| {
                (
                    PolicyFile::ALL.iter().position(|f| *f == p.file),
                    p.line,
                    p.column,
                )
            });
            PolicyError::Invalid { problems }
        })
    }

    fn path(&self, file: PolicyFile) -> PathBuf {
        self.dir.join(file.file_name())
    }

    fn content(&self, file: PolicyFile) -> Option<&[u8]> {
        let index = PolicyFile::ALL.iter().position(|f| *f == file)?;
        self.bytes[index].as_deref()
    }

    /// Parses one file into `T`. Returns `Some(None)` when the file is absent and `None` (after
    /// recording the problem) when it is invalid.
    fn parse<T: DeserializeOwned>(
        &self,
        file: PolicyFile,
        problems: &mut Vec<Problem>,
    ) -> Option<Option<(T, Presence)>> {
        let Some(bytes) = self.content(file) else {
            return Some(None);
        };
        match parse_document::<T>(file, self.path(file), bytes) {
            Ok(parsed) => Some(Some(parsed)),
            Err(problem) => {
                problems.push(problem);
                None
            }
        }
    }

    /// Attaches a file position to a semantic problem.
    fn locate(&self, found: Found) -> Problem {
        let text = self
            .content(found.file)
            .and_then(|b| std::str::from_utf8(b).ok())
            .map(strip_bom);
        let position = text.and_then(|text| {
            let root = DeTable::parse(text).ok()?;
            let span = locate(&root, &found.at)?;
            Some(line_column(text, span.start))
        });
        Problem {
            file: found.file,
            path: self.path(found.file),
            kind: found.kind,
            line: position.map(|(line, _)| line),
            column: position.map(|(_, column)| column),
            message: found.message,
            fix: found.fix,
        }
    }
}

fn strip_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

/// Parses a file into `T`; `Presence` is only meaningful for `guards.toml`.
fn parse_document<T: DeserializeOwned>(
    file: PolicyFile,
    path: PathBuf,
    bytes: &[u8],
) -> Result<(T, Presence), Problem> {
    let problem = |kind, span: Option<Range<usize>>, text: &str, message: String, fix: String| {
        let position = span.map(|span| line_column(text, span.start));
        Problem {
            file,
            path: path.clone(),
            kind,
            line: position.map(|(line, _)| line),
            column: position.map(|(_, column)| column),
            message,
            fix,
        }
    };
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => strip_bom(text),
        Err(err) => {
            return Err(problem(
                ProblemKind::Encoding,
                None,
                "",
                format!(
                    "the file is not valid UTF-8 (invalid byte at offset {})",
                    err.valid_up_to()
                ),
                "save the file as UTF-8".to_owned(),
            ));
        }
    };
    let root = match DeTable::parse(text) {
        Ok(root) => root,
        Err(err) => {
            let fix = if err.message().contains("duplicate key") {
                "keys and tables may appear only once; merge or rename the duplicate"
            } else {
                "fix the TOML syntax at this position (quotes, brackets and `=` are the usual suspects)"
            };
            return Err(problem(
                ProblemKind::Syntax,
                err.span(),
                text,
                err.message().to_owned(),
                fix.to_owned(),
            ));
        }
    };
    let table = root.get_ref();
    let presence = Presence {
        protected_branches: table.contains_key("protected_branches"),
        ban: table
            .get("destructive")
            .is_some_and(|d| d.get_ref().get("ban").is_some()),
    };
    match toml::from_str::<T>(text) {
        Ok(value) => Ok((value, presence)),
        Err(err) => Err(problem(
            ProblemKind::Schema,
            err.span(),
            text,
            err.message().to_owned(),
            schema_fix(file, err.message()),
        )),
    }
}

/// A fix hint for a serde error message.
fn schema_fix(file: PolicyFile, message: &str) -> String {
    let name = file.file_name();
    if message.starts_with("unknown field") {
        format!(
            "remove the key or fix its spelling; {name} only accepts the keys listed in the message"
        )
    } else if message.starts_with("missing field") && file == PolicyFile::Config {
        "add `spec = \"acs/0.1\"` at the top of the file".to_owned()
    } else if message.starts_with("missing field") {
        "add the missing key (see SPEC.md section 5)".to_owned()
    } else if message.contains("unsupported spec version") {
        "set `spec = \"acs/0.1\"`, the only version this release understands".to_owned()
    } else if message.starts_with("unknown variant") {
        "use one of the values listed in the message".to_owned()
    } else {
        format!("correct the value so it matches the schema in SPEC.md section 5 ({name})")
    }
}

/// Finds the span of `at` in a parsed document, falling back to the closest enclosing element.
fn locate(root: &Spanned<DeTable<'_>>, at: &At) -> Option<Range<usize>> {
    let table = root.get_ref();
    let key = |name: &str| table.get(name);
    // The `i`-th element of the array under `field` of `parent`, or the array itself.
    let element = |parent: &Spanned<DeValue<'_>>, field: &str, i: usize| {
        let array = parent.get_ref().get(field)?;
        Some(
            array
                .get_ref()
                .get(i)
                .map_or_else(|| array.span(), Spanned::span),
        )
    };
    let zone = |i: usize| key("zone").and_then(|zones| zones.get_ref().get(i));
    let zone_field = |i: usize, field: &str| {
        let zone = zone(i)?;
        Some(
            zone.get_ref()
                .get(field)
                .map_or_else(|| zone.span(), Spanned::span),
        )
    };
    match at {
        At::TargetBranch => key("default_target_branch").map(Spanned::span),
        At::Sealed(glob) => {
            let sealed = key("sealed")?;
            let position = (0..)
                .map_while(|i| sealed.get_ref().get(i))
                .position(|item| item.get_ref().as_str() == Some(glob.as_str()))?;
            Some(element_of(sealed, position))
        }
        At::Protected(i) => Some(element_of(key("protected_branches")?, *i)),
        At::Ban(i) => element(key("destructive")?, "ban", *i),
        At::ZoneKind(i) => zone_field(*i, "kind"),
        At::ZonePaths(i) => zone_field(*i, "paths"),
        At::ZonePath(i, j) => element(zone(*i)?, "paths", *j),
        At::ZoneOwner(i, j) => element(zone(*i)?, "owners", *j),
        At::Team(name) => {
            let teams = key("teams")?;
            match teams.get_ref() {
                DeValue::Table(t) => t
                    .get_key_value(name.as_str())
                    .map(|(k, _)| k.span())
                    .or(Some(teams.span())),
                _ => Some(teams.span()),
            }
        }
        At::TeamMember(name, j) => {
            let members = key("teams")?.get_ref().get(name.as_str())?;
            Some(element_of(members, *j))
        }
    }
}

/// The span of element `i` of an array value, or of the whole value if it is not an array.
fn element_of(array: &Spanned<DeValue<'_>>, i: usize) -> Range<usize> {
    array
        .get_ref()
        .get(i)
        .map_or_else(|| array.span(), Spanned::span)
}

/// The 1-based line and column (in characters) of a byte offset.
fn line_column(text: &str, offset: usize) -> (usize, usize) {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    let before = &text[..offset];
    let line = before.matches('\n').count() + 1;
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    (line, before[line_start..].chars().count() + 1)
}

/// The policy hash: SHA-256 over the three files, in the order config, guards, territory.
///
/// Each file contributes its name and either an absent marker or its length and bytes, so
/// moving content between files, or an absent file versus an empty one, changes the hash.
pub(crate) fn hash_files(files: &[Option<Vec<u8>>; 3]) -> ContentHash {
    let mut buffer = HASH_DOMAIN.to_vec();
    for (file, bytes) in PolicyFile::ALL.iter().zip(files) {
        buffer.extend_from_slice(file.file_name().as_bytes());
        buffer.push(0);
        match bytes {
            None => buffer.push(0),
            Some(bytes) => {
                buffer.push(1);
                buffer.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
                buffer.extend_from_slice(bytes);
            }
        }
    }
    ContentHash::of(&buffer)
}
