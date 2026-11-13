//! Words, working-directory tracking and lexical path helpers shared by every dialect.

use acs_core::gate::{Confidence, GitInvocation, ShellTarget, TargetOp};

/// How much is known about the value of a word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// A literal after quote removal.
    Exact,
    /// Contains an unquoted glob character or brace expansion.
    Pattern,
    /// Contains an unexpanded variable, command substitution or a leading `~`.
    Partial,
    /// Not a path at all (process substitution, script block, `find` placeholder).
    Opaque,
}

/// One shell word after quote removal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Word {
    pub text: String,
    pub kind: Kind,
}

impl Word {
    pub fn exact(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            kind: Kind::Exact,
        }
    }

    pub fn opaque(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            kind: Kind::Opaque,
        }
    }

    /// Whether the word is fully known (so can be compared with program or option names).
    pub fn is_exact(&self) -> bool {
        self.kind == Kind::Exact
    }

    /// Whether the word reads as an option (`-x`, `--long`, but not a lone `-`).
    pub fn is_option(&self) -> bool {
        self.is_exact() && self.text.len() > 1 && self.text.starts_with('-')
    }
}

/// A redirection to or from a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Redirect {
    pub op: TargetOp,
    pub word: Word,
}

/// The directory relative paths are resolved against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Cwd {
    /// A literal directory relative to where the command line started (`""` is the start itself)
    /// or an absolute one.
    Known(String),
    /// A directory that cannot be determined (`cd "$X"`, `cd`, `cd -`).
    Unknown,
}

impl Cwd {
    /// Where the command line starts.
    pub fn start() -> Self {
        Self::Known(String::new())
    }

    /// The directory after `cd <dir>`.
    pub fn change(&self, dir: Option<&Word>) -> Self {
        let Some(dir) = dir else {
            return Self::Unknown;
        };
        if !dir.is_exact() || dir.text == "-" {
            return Self::Unknown;
        }
        if dir.text.is_empty() {
            return self.clone();
        }
        if is_absolute(&dir.text) {
            return Self::Known(normalize(&dir.text));
        }
        match self {
            Self::Known(base) => match join(base, &dir.text).as_str() {
                "." => Self::start(),
                joined => Self::Known(joined.to_string()),
            },
            Self::Unknown => Self::Unknown,
        }
    }

    /// Resolves a target path written relative to this directory.
    pub fn apply(&self, target: &mut ShellTarget) {
        if is_absolute(&target.path) || target.confidence == Confidence::Partial {
            return;
        }
        match self {
            Self::Known(base) if base.is_empty() => {}
            Self::Known(base) => target.path = join(base, &target.path),
            Self::Unknown => target.confidence = Confidence::Partial,
        }
    }

    /// Places the directory a `git` invocation runs in (its own `-C` chain so far) under this
    /// directory.
    pub fn locate(&self, git: &mut GitInvocation) {
        if git.dir_unknown || is_absolute(&git.dir) {
            return;
        }
        match self {
            Self::Known(base) if git.dir.is_empty() => git.dir.clone_from(base),
            Self::Known(base) => {
                git.dir = join(base, &git.dir);
                if git.dir == "." {
                    git.dir.clear();
                }
            }
            Self::Unknown => {
                git.dir_unknown = true;
                git.dir.clear();
            }
        }
    }
}

/// Whether a path is absolute on POSIX or Windows.
pub(crate) fn is_absolute(path: &str) -> bool {
    let bytes = path.as_bytes();
    path.starts_with('/')
        || path.starts_with('\\')
        || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
}

/// Joins `rel` onto `base` and lexically normalizes the result.
pub(crate) fn join(base: &str, rel: &str) -> String {
    if base.is_empty() {
        return normalize(rel);
    }
    normalize(&format!("{base}/{rel}"))
}

/// Lexical normalization: collapses `.`, `..` (where possible) and repeated separators. A
/// trailing separator is kept, and Windows separators are treated like `/`.
pub(crate) fn normalize(path: &str) -> String {
    let unified = path.replace('\\', "/");
    let absolute = unified.starts_with('/');
    let trailing = unified.len() > 1 && unified.ends_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for segment in unified.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|last| *last != "..") {
                    parts.pop();
                } else if !absolute {
                    parts.push("..");
                }
            }
            other => parts.push(other),
        }
    }
    let mut out = parts.join("/");
    if absolute {
        out.insert(0, '/');
    }
    if out.is_empty() {
        return ".".to_string();
    }
    if trailing && out != "/" {
        out.push('/');
    }
    out
}

/// The last component of a path (`a/b/` gives `b`).
pub(crate) fn basename(path: &str) -> &str {
    path.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
}

/// The directory part of a path (`a/b` gives `a`, `b` gives `.`).
pub(crate) fn dirname(path: &str) -> String {
    let trimmed = path.trim_end_matches(['/', '\\']);
    match trimmed.rfind(['/', '\\']) {
        Some(0) => "/".to_string(),
        Some(index) => trimmed[..index].to_string(),
        None => ".".to_string(),
    }
}

/// Whether writing to this path has no effect on any file.
pub(crate) fn is_null_device(path: &str) -> bool {
    matches!(
        path,
        "/dev/null" | "/dev/stdout" | "/dev/stderr" | "/dev/stdin" | "/dev/tty"
    ) || path.starts_with("/dev/fd/")
}

/// The confidence a word gives a target, or `None` when the word is not a path.
pub(crate) fn confidence(kind: Kind) -> Option<Confidence> {
    match kind {
        Kind::Exact => Some(Confidence::Exact),
        Kind::Pattern => Some(Confidence::Pattern),
        Kind::Partial => Some(Confidence::Partial),
        Kind::Opaque => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_lexically() {
        assert_eq!(normalize("a/./b//c/"), "a/b/c/");
        assert_eq!(normalize("a/../b"), "b");
        assert_eq!(normalize("../a"), "../a");
        assert_eq!(normalize("a/../../b"), "../b");
        assert_eq!(normalize("/a/../.."), "/");
        assert_eq!(normalize("a/.."), ".");
        assert_eq!(normalize("C:\\a\\b"), "C:/a/b");
    }

    #[test]
    fn splits_paths() {
        assert_eq!(basename("a/b/"), "b");
        assert_eq!(basename("b"), "b");
        assert_eq!(dirname("a/b"), "a");
        assert_eq!(dirname("/b"), "/");
        assert_eq!(dirname("b"), ".");
    }

    #[test]
    fn tracks_directory_changes() {
        let start = Cwd::start();
        let sub = start.change(Some(&Word::exact("sub")));
        assert_eq!(sub, Cwd::Known("sub".into()));
        assert_eq!(
            sub.change(Some(&Word::exact(".."))),
            Cwd::start(),
            "cd .. back to the start is the start again"
        );
        assert_eq!(sub.change(None), Cwd::Unknown);
        assert_eq!(
            sub.change(Some(&Word::exact("/tmp"))),
            Cwd::Known("/tmp".into())
        );
        assert_eq!(Cwd::Unknown.change(Some(&Word::exact("x"))), Cwd::Unknown);
    }
}
