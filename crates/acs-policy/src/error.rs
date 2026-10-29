//! Errors of the policy loader: which file, where, what is wrong and how to fix it.

use std::fmt;
use std::io;
use std::path::PathBuf;

/// One of the three policy files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PolicyFile {
    /// `config.toml`
    Config,
    /// `guards.toml`
    Guards,
    /// `territory.toml`
    Territory,
}

impl PolicyFile {
    /// The three files in the order they are hashed and reported.
    pub const ALL: [Self; 3] = [Self::Config, Self::Guards, Self::Territory];

    /// The file's name inside the policy directory.
    #[must_use]
    pub const fn file_name(self) -> &'static str {
        match self {
            Self::Config => "config.toml",
            Self::Guards => "guards.toml",
            Self::Territory => "territory.toml",
        }
    }
}

/// What kind of mistake a [`Problem`] reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProblemKind {
    /// The file is not valid UTF-8.
    Encoding,
    /// The file is not valid TOML.
    Syntax,
    /// Valid TOML that does not fit the file's schema: an unknown or missing key, a value of the
    /// wrong type, an unsupported spec version.
    Schema,
    /// A value that fits the schema but is not acceptable (for example an empty
    /// `default_target_branch`).
    InvalidValue,
    /// A path or branch glob that does not compile.
    InvalidGlob,
    /// A `[[zone]]` with an empty `paths` list.
    EmptyZonePaths,
    /// An `owned` zone without `owners`.
    OwnedWithoutOwners,
    /// An owner `team:<name>` that `[teams]` does not define.
    UndefinedTeam,
    /// Two teams whose names differ only in case.
    DuplicateTeam,
    /// A team name that `team:<name>` could never refer to.
    InvalidTeamName,
    /// A team member that is not an email address.
    InvalidTeamMember,
    /// An empty `destructive.ban` pattern.
    EmptyBan,
    /// A `destructive.ban` pattern that would match every command.
    BanTooBroad,
}

/// One thing wrong with a policy file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// The file.
    pub file: PolicyFile,
    /// The file's path (the policy directory joined with its name).
    pub path: PathBuf,
    /// What kind of mistake it is.
    pub kind: ProblemKind,
    /// The 1-based line, when known.
    pub line: Option<usize>,
    /// The 1-based column (in characters), when known.
    pub column: Option<usize>,
    /// What is wrong.
    pub message: String,
    /// How to fix it.
    pub fix: String,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.path.display())?;
        match (self.line, self.column) {
            (Some(line), Some(column)) => write!(f, ":{line}:{column}")?,
            (Some(line), None) => write!(f, ":{line}")?,
            _ => {}
        }
        write!(f, ": {} (fix: {})", self.message, self.fix)
    }
}

/// Why a policy could not be loaded.
#[derive(Debug, thiserror::Error)]
pub enum PolicyError {
    /// A policy file exists but could not be read.
    #[error("cannot read policy file {}: {source}", path.display())]
    Io {
        /// The file.
        path: PathBuf,
        /// The underlying error.
        source: io::Error,
    },
    /// The policy files are invalid. Every problem found is listed.
    #[error("{}", render_problems(problems))]
    Invalid {
        /// The problems, ordered by file (config, guards, territory) and position.
        problems: Vec<Problem>,
    },
}

fn render_problems(problems: &[Problem]) -> String {
    let mut text = match problems {
        [_] => "invalid policy: ".to_owned(),
        _ => format!("invalid policy, {} problems:\n  ", problems.len()),
    };
    let lines: Vec<String> = problems.iter().map(ToString::to_string).collect();
    text.push_str(&lines.join("\n  "));
    text
}

impl PolicyError {
    /// The problems, if the policy was readable but invalid.
    #[must_use]
    pub fn problems(&self) -> &[Problem] {
        match self {
            Self::Invalid { problems } => problems,
            Self::Io { .. } => &[],
        }
    }
}
