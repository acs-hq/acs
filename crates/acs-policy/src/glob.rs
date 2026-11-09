//! The spec's glob dialect (SPEC section 5), compiled to a small matcher.
//!
//! The dialect is deliberately tiny, so it is implemented here instead of through a regex engine:
//! compiling a pattern costs microseconds, which keeps loading a cached policy well under a
//! millisecond, and the semantics (including `**` matching zero segments) are exactly the spec's.
//!
//! - Patterns and paths are `/`-separated and the pattern is anchored at the root of the path.
//! - `*` matches any run of characters within one segment, `?` one character within a segment.
//! - `**` as a whole segment matches zero or more segments; `a/**` therefore also matches `a`.
//! - `[abc]` and `[a-z]` match one character of a set; there is no negation, and `/` is not
//!   allowed inside a set. A `]` directly after `[` is a member of the set.
//! - `{a,b}` matches any of its comma-separated alternatives, which may contain `/` and may be
//!   nested. A `,` outside braces is literal.
//! - There is no escape character: to match a literal `* ? [ ] { }`, put it in a set (`[*]`,
//!   `[{]`). A backslash is rejected, because it is almost always a Windows path separator.
//! - Case-insensitive matching lowercases the pattern and the path.

use std::borrow::Cow;
use std::cmp::Reverse;

/// The largest number of alternatives a single pattern may expand to through `{a,b}`.
const MAX_ALTERNATIVES: usize = 64;

/// Why a glob pattern was rejected, and how to fix it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct GlobError {
    /// What is wrong with the pattern.
    pub message: String,
    /// How to fix it.
    pub fix: String,
}

impl GlobError {
    fn new(message: impl Into<String>, fix: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            fix: fix.into(),
        }
    }
}

/// A compiled glob pattern.
///
/// Compile with [`Glob::new`] and test paths with [`Glob::is_match`]. Paths are
/// repository-relative and `/`-separated.
#[derive(Debug, Clone)]
pub struct Glob {
    alts: Vec<Alt>,
    case_insensitive: bool,
}

impl Glob {
    /// Compiles `pattern`. With `case_insensitive`, [`Glob::is_match`] ignores case.
    ///
    /// # Errors
    ///
    /// Returns a [`GlobError`] naming what is wrong: an empty pattern or segment, a leading `/`,
    /// a `.` or `..` segment, a backslash, `**` inside a segment, an unclosed or negated set, an
    /// unbalanced brace, or too many `{}` alternatives.
    pub fn new(pattern: &str, case_insensitive: bool) -> Result<Self, GlobError> {
        let folded = if case_insensitive {
            fold(pattern)
        } else {
            Cow::Borrowed(pattern)
        };
        let alts = expand(&folded)?
            .iter()
            .map(|alt| Alt::parse(alt))
            .collect::<Result<_, _>>()?;
        Ok(Self {
            alts,
            case_insensitive,
        })
    }

    /// Whether `path` matches the pattern.
    #[must_use]
    pub fn is_match(&self, path: &str) -> bool {
        with_segments(path, self.case_insensitive, |segs| {
            self.alts.iter().any(|alt| alt.is_match(segs))
        })
    }

    /// How specifically the pattern matches `path`: the [`Specificity`] of the most specific
    /// `{}` alternative that matches, or `None` when the pattern does not match at all.
    #[must_use]
    pub fn match_specificity(&self, path: &str) -> Option<Specificity> {
        with_segments(path, self.case_insensitive, |segs| {
            self.alts
                .iter()
                .filter(|alt| alt.is_match(segs))
                .map(Alt::specificity)
                .max()
        })
    }

    /// The compiled alternatives, one per `{}` expansion.
    pub(crate) fn alts(&self) -> &[Alt] {
        &self.alts
    }

    /// Consumes the glob, returning its alternatives.
    pub(crate) fn into_alts(self) -> Vec<Alt> {
        self.alts
    }
}

/// Runs `f` on the `/`-separated segments of `path`, lowercased when `case_insensitive`.
pub(crate) fn with_segments<R>(
    path: &str,
    case_insensitive: bool,
    f: impl FnOnce(&[&str]) -> R,
) -> R {
    let path = if case_insensitive {
        fold(path)
    } else {
        Cow::Borrowed(path)
    };
    let segs: Vec<&str> = path.split('/').collect();
    f(&segs)
}

/// Lowercases `text` character by character (no context-dependent mappings), borrowing when it
/// is already lowercase.
pub(crate) fn fold(text: &str) -> Cow<'_, str> {
    let unchanged = text.chars().all(|c| {
        let mut lower = c.to_lowercase();
        lower.next() == Some(c) && lower.next().is_none()
    });
    if unchanged {
        Cow::Borrowed(text)
    } else {
        Cow::Owned(text.chars().flat_map(char::to_lowercase).collect())
    }
}

/// How specific an alternative is; larger is more specific. Compared field by field:
/// leading literal segments, fewer `**` segments, more literal characters, fewer wildcards.
/// The same ranking picks the winning zone (see [`crate::CompiledPolicy`]) and orders lore.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Specificity {
    leading_literal_segments: usize,
    fewer_globstars: Reverse<usize>,
    literal_chars: usize,
    fewer_wildcards: Reverse<usize>,
}

/// One brace-free pattern: a sequence of segments.
#[derive(Debug, Clone)]
pub(crate) struct Alt {
    segs: Vec<Seg>,
    specificity: Specificity,
}

#[derive(Debug, Clone)]
enum Seg {
    /// `**`: zero or more whole segments.
    Globstar,
    /// A segment without wildcards.
    Literal(String),
    /// A segment with at least one wildcard or set.
    Pattern(Vec<Tok>),
}

#[derive(Debug, Clone)]
enum Tok {
    Char(char),
    /// `?`
    AnyChar,
    /// `*`
    AnyRun,
    /// `[...]`: inclusive ranges (a single member is a one-character range).
    Set(Vec<(char, char)>),
}

impl Tok {
    fn accepts(&self, c: char) -> bool {
        match self {
            Self::Char(expected) => *expected == c,
            Self::AnyChar => true,
            Self::AnyRun => false,
            Self::Set(ranges) => ranges.iter().any(|&(lo, hi)| lo <= c && c <= hi),
        }
    }
}

impl Alt {
    /// Parses a brace-free pattern.
    fn parse(pattern: &str) -> Result<Self, GlobError> {
        if pattern.is_empty() {
            return Err(GlobError::new(
                "the pattern is empty",
                "write a path glob such as \"src/**\"",
            ));
        }
        if pattern.starts_with('/') {
            return Err(GlobError::new(
                "the pattern starts with \"/\"",
                "patterns are already anchored at the repository root; drop the leading \"/\"",
            ));
        }
        let mut segs = Vec::new();
        for raw in pattern.split('/') {
            segs.push(parse_segment(raw)?);
        }
        let specificity = Specificity {
            leading_literal_segments: segs
                .iter()
                .take_while(|s| matches!(s, Seg::Literal(_)))
                .count(),
            fewer_globstars: Reverse(segs.iter().filter(|s| matches!(s, Seg::Globstar)).count()),
            literal_chars: segs
                .iter()
                .map(|s| match s {
                    Seg::Globstar => 0,
                    Seg::Literal(text) => text.chars().count(),
                    Seg::Pattern(toks) => toks.iter().filter(|t| matches!(t, Tok::Char(_))).count(),
                })
                .sum(),
            fewer_wildcards: Reverse(
                segs.iter()
                    .map(|s| match s {
                        Seg::Pattern(toks) => {
                            toks.iter().filter(|t| !matches!(t, Tok::Char(_))).count()
                        }
                        Seg::Globstar | Seg::Literal(_) => 0,
                    })
                    .sum(),
            ),
        };
        Ok(Self { segs, specificity })
    }

    pub(crate) fn specificity(&self) -> Specificity {
        self.specificity
    }

    /// Matches pre-split (and, if needed, pre-folded) path segments.
    pub(crate) fn is_match(&self, path: &[&str]) -> bool {
        // Iterative matching with backtracking to the most recent `**`; linear in the product of
        // the lengths, so hostile paths cannot make it blow up.
        let (mut p, mut s) = (0, 0);
        let mut globstar: Option<(usize, usize)> = None;
        loop {
            if let Some(seg) = self.segs.get(p) {
                if matches!(seg, Seg::Globstar) {
                    globstar = Some((p, s));
                    p += 1;
                    continue;
                }
                if path.get(s).is_some_and(|text| seg.is_match(text)) {
                    p += 1;
                    s += 1;
                    continue;
                }
            } else if s == path.len() {
                return true;
            }
            match globstar {
                Some((at, consumed)) if consumed < path.len() => {
                    globstar = Some((at, consumed + 1));
                    p = at + 1;
                    s = consumed + 1;
                }
                _ => return false,
            }
        }
    }
}

impl Seg {
    fn is_match(&self, text: &str) -> bool {
        match self {
            Self::Globstar => true,
            Self::Literal(literal) => literal == text,
            Self::Pattern(toks) => tokens_match(toks, text),
        }
    }
}

/// Matches `text` against wildcard tokens, backtracking only to the latest `*`.
fn tokens_match(toks: &[Tok], text: &str) -> bool {
    let (mut p, mut t) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    loop {
        if let Some(tok) = toks.get(p) {
            if matches!(tok, Tok::AnyRun) {
                star = Some((p + 1, t));
                p += 1;
                continue;
            }
            if let Some(c) = text[t..].chars().next()
                && tok.accepts(c)
            {
                p += 1;
                t += c.len_utf8();
                continue;
            }
        } else if t == text.len() {
            return true;
        }
        let Some((after_star, from)) = star else {
            return false;
        };
        let Some(c) = text[from..].chars().next() else {
            return false;
        };
        star = Some((after_star, from + c.len_utf8()));
        p = after_star;
        t = from + c.len_utf8();
    }
}

fn parse_segment(raw: &str) -> Result<Seg, GlobError> {
    match raw {
        "" => {
            return Err(GlobError::new(
                "the pattern has an empty path segment (a doubled or trailing \"/\")",
                "remove the extra \"/\"; to cover everything under a directory write \"dir/**\"",
            ));
        }
        "**" => return Ok(Seg::Globstar),
        "." | ".." => {
            return Err(GlobError::new(
                format!(
                    "the pattern has a \"{raw}\" segment, which never matches a normalized path"
                ),
                "write the path relative to the repository root without \"./\" or \"../\"",
            ));
        }
        _ if raw.contains("**") => {
            return Err(GlobError::new(
                format!("\"**\" must be a whole path segment, found it inside \"{raw}\""),
                "put \"**\" between slashes (\"a/**/b\"), or use a single \"*\" within a segment",
            ));
        }
        _ => {}
    }
    let chars: Vec<char> = raw.chars().collect();
    let mut toks = Vec::new();
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        match c {
            '*' => {
                if !matches!(toks.last(), Some(Tok::AnyRun)) {
                    toks.push(Tok::AnyRun);
                }
            }
            '?' => toks.push(Tok::AnyChar),
            '[' => {
                let end = scan_set(&chars, i)?;
                toks.push(Tok::Set(parse_set(&chars[i + 1..end])?));
                i = end;
            }
            _ => toks.push(Tok::Char(c)),
        }
        i += 1;
    }
    if toks.iter().all(|t| matches!(t, Tok::Char(_))) {
        Ok(Seg::Literal(chars.into_iter().collect()))
    } else {
        Ok(Seg::Pattern(toks))
    }
}

/// Parses the members of a set (the text between `[` and `]`).
fn parse_set(members: &[char]) -> Result<Vec<(char, char)>, GlobError> {
    let mut ranges = Vec::new();
    let mut i = 0;
    while let Some(&lo) = members.get(i) {
        if members.get(i + 1) == Some(&'-')
            && let Some(&hi) = members.get(i + 2)
        {
            if lo > hi {
                return Err(GlobError::new(
                    format!("the range \"{lo}-{hi}\" in a [...] set is reversed"),
                    format!("write it as \"{hi}-{lo}\""),
                ));
            }
            ranges.push((lo, hi));
            i += 3;
        } else {
            ranges.push((lo, lo));
            i += 1;
        }
    }
    Ok(ranges)
}

/// Finds the `]` closing the set that opens at `chars[open]`.
fn scan_set(chars: &[char], open: usize) -> Result<usize, GlobError> {
    let mut i = open + 1;
    if matches!(chars.get(i), Some('!' | '^')) {
        return Err(GlobError::new(
            "negated sets ([!...] or [^...]) are not supported",
            "list the characters you want instead; the dialect has no negation",
        ));
    }
    if chars.get(i) == Some(&']') {
        i += 1;
    }
    while let Some(&c) = chars.get(i) {
        match c {
            ']' => return Ok(i),
            '/' => {
                return Err(GlobError::new(
                    "a [...] set cannot contain \"/\"",
                    "use separate path segments instead",
                ));
            }
            _ => i += 1,
        }
    }
    Err(GlobError::new(
        "a \"[\" is never closed",
        "add the closing \"]\", or write a literal \"[\" as \"[[]\"",
    ))
}

/// Expands `{a,b}` groups into brace-free patterns.
fn expand(pattern: &str) -> Result<Vec<String>, GlobError> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut pos = 0;
    expand_sequence(&chars, &mut pos, false)
}

/// Expands text up to the end, or (inside a group) up to the `,` or `}` that ends the
/// alternative, which is left unconsumed.
fn expand_sequence(
    chars: &[char],
    pos: &mut usize,
    in_group: bool,
) -> Result<Vec<String>, GlobError> {
    let mut acc = vec![String::new()];
    while let Some(&c) = chars.get(*pos) {
        match c {
            '\\' => {
                return Err(GlobError::new(
                    "the pattern contains a backslash",
                    "use \"/\" as the path separator; to match a literal special character \
                     put it in a set, such as \"[*]\"",
                ));
            }
            '{' => {
                *pos += 1;
                let group = expand_group(chars, pos)?;
                if acc.len() * group.len() > MAX_ALTERNATIVES {
                    return Err(too_many());
                }
                acc = acc
                    .iter()
                    .flat_map(|prefix| group.iter().map(move |alt| format!("{prefix}{alt}")))
                    .collect();
            }
            ',' | '}' if in_group => return Ok(acc),
            '}' => {
                return Err(GlobError::new(
                    "a \"}\" has no matching \"{\"",
                    "remove it, or write a literal \"}\" as \"[}]\"",
                ));
            }
            '[' => {
                let end = scan_set(chars, *pos)?;
                let set: String = chars[*pos..=end].iter().collect();
                for text in &mut acc {
                    text.push_str(&set);
                }
                *pos = end + 1;
            }
            _ => {
                for text in &mut acc {
                    text.push(c);
                }
                *pos += 1;
            }
        }
    }
    if in_group {
        return Err(unclosed_brace());
    }
    Ok(acc)
}

/// Expands a group after its `{`, consuming the closing `}`.
fn expand_group(chars: &[char], pos: &mut usize) -> Result<Vec<String>, GlobError> {
    let mut all = Vec::new();
    loop {
        all.extend(expand_sequence(chars, pos, true)?);
        if all.len() > MAX_ALTERNATIVES {
            return Err(too_many());
        }
        match chars.get(*pos) {
            Some(',') => *pos += 1,
            Some('}') => {
                *pos += 1;
                return Ok(all);
            }
            _ => return Err(unclosed_brace()),
        }
    }
}

fn unclosed_brace() -> GlobError {
    GlobError::new(
        "a \"{\" is never closed",
        "add the closing \"}\", or write a literal \"{\" as \"[{]\"",
    )
}

fn too_many() -> GlobError {
    GlobError::new(
        format!("the {{...}} groups expand to more than {MAX_ALTERNATIVES} patterns"),
        "split the pattern into several entries",
    )
}

/// A `destructive.ban` pattern: `*` matches any run of characters (including `/` and spaces),
/// everything else is literal, and the whole command must match.
#[derive(Debug, Clone)]
pub(crate) struct BanPattern {
    toks: Vec<Tok>,
}

impl BanPattern {
    /// Compiles a ban pattern. Surrounding whitespace is dropped and runs of whitespace become a
    /// single space, because commands are matched with their words joined by single spaces.
    pub(crate) fn new(pattern: &str) -> Self {
        let normalized = pattern.split_whitespace().collect::<Vec<_>>().join(" ");
        let mut toks = Vec::new();
        for c in normalized.chars() {
            if c == '*' {
                if !matches!(toks.last(), Some(Tok::AnyRun)) {
                    toks.push(Tok::AnyRun);
                }
            } else {
                toks.push(Tok::Char(c));
            }
        }
        Self { toks }
    }

    /// Whether the pattern consists only of `*` (or nothing), which would ban every command.
    pub(crate) fn matches_everything(&self) -> bool {
        self.toks.iter().all(|t| matches!(t, Tok::AnyRun))
    }

    /// Whether the command (its words joined by single spaces) matches.
    pub(crate) fn is_match(&self, command: &str) -> bool {
        tokens_match(&self.toks, command)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glob(pattern: &str) -> Glob {
        Glob::new(pattern, false).unwrap_or_else(|e| panic!("{pattern:?}: {e}"))
    }

    #[test]
    fn matching_table() {
        // (pattern, path, expected)
        let cases: &[(&str, &str, bool)] = &[
            // literals and anchoring
            ("src/lib.rs", "src/lib.rs", true),
            ("src/lib.rs", "x/src/lib.rs", false),
            ("src", "src/lib.rs", false),
            ("src/lib.rs", "src/lib.rsx", false),
            // `*` and `?` stay within a segment
            ("src/*.rs", "src/lib.rs", true),
            ("src/*.rs", "src/a/lib.rs", false),
            ("*", "a", true),
            ("*", "a/b", false),
            ("*.md", ".md", true),
            ("a?c", "abc", true),
            ("a?c", "ac", false),
            ("a?c", "a/c", false),
            ("a*b*c", "aXbYc", true),
            ("a*b*c", "abc", true),
            ("a*b*c", "aXbY", false),
            (
                "*a*a*a*a*b",
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                false,
            ),
            ("é?", "éa", true),
            // `**` as a segment
            ("**", "a", true),
            ("**", "a/b/c", true),
            ("src/**", "src/a", true),
            ("src/**", "src/a/b/c.rs", true),
            ("src/**", "src", true),
            ("src/**", "srcx/a", false),
            ("src/**", "other/src/a", false),
            ("**/x", "x", true),
            ("**/x", "a/b/x", true),
            ("**/x", "a/b/xy", false),
            ("a/**/b", "a/b", true),
            ("a/**/b", "a/x/y/b", true),
            ("a/**/b", "a/x/y/c", false),
            ("a/**/b/**/c", "a/b/c", true),
            ("a/**/b/**/c", "a/1/b/2/3/c", true),
            ("a/**/b/**/c", "a/1/c/b", false),
            ("**/.env*", ".env", true),
            ("**/.env*", "app/.env.local", true),
            ("**/.env*", "app/env", false),
            (".git/**", ".git/config", true),
            (".git/**", ".github/workflows/x", false),
            (
                ".claude/settings*.json",
                ".claude/settings.local.json",
                true,
            ),
            (".claude/settings*.json", ".claude/sub/settings.json", false),
            (".agents/**/hooks*.json", ".agents/x/y/hooks.json", true),
            (".agents/**/hooks*.json", ".agents/hooks-a.json", true),
            // sets
            ("file[0-9].txt", "file7.txt", true),
            ("file[0-9].txt", "filex.txt", false),
            ("[abc]", "b", true),
            ("[abc]", "d", false),
            ("[]]", "]", true),
            ("[a-]", "-", true),
            ("[*]", "*", true),
            ("[*]", "a", false),
            ("[{]x", "{x", true),
            // braces
            ("*.{rs,toml}", "a.rs", true),
            ("*.{rs,toml}", "a.toml", true),
            ("*.{rs,toml}", "a.md", false),
            ("{src,lib}/**", "lib/a", true),
            ("{src,lib}/**", "bin/a", false),
            ("{a/b,c}/d", "a/b/d", true),
            ("{a/b,c}/d", "c/d", true),
            ("{a/b,c}/d", "a/d", false),
            ("a{,b}c", "ac", true),
            ("a{,b}c", "abc", true),
            ("a{b,{c,d}}e", "ade", true),
            ("a{b,{c,d}}e", "ae", false),
            ("a,b", "a,b", true),
            ("{**,x}/y", "q/y", true),
        ];
        for &(pattern, path, expected) in cases {
            assert_eq!(
                glob(pattern).is_match(path),
                expected,
                "{pattern:?} vs {path:?}"
            );
        }
    }

    #[test]
    fn case_folding() {
        let ci = |p: &str| Glob::new(p, true).unwrap();
        assert!(ci("Src/**").is_match("sRC/Main.RS"));
        assert!(ci("**/.ENV*").is_match("app/.env"));
        assert!(ci("[A-C]x").is_match("bX"));
        assert!(ci("ÄB").is_match("äb"));
        assert!(!glob("Src/**").is_match("src/a"));
        assert!(!glob("[A-C]x").is_match("bx"));
    }

    #[test]
    fn invalid_patterns() {
        let cases: &[(&str, &str)] = &[
            ("", "empty"),
            ("/src/**", "starts with"),
            ("./src", "\".\" segment"),
            ("a/../b", "\"..\" segment"),
            ("a//b", "empty path segment"),
            ("a/", "empty path segment"),
            ("src\\lib.rs", "backslash"),
            ("a**b", "whole path segment"),
            ("***/x", "whole path segment"),
            ("a/[b", "never closed"),
            ("[!a]", "negated"),
            ("[^a]", "negated"),
            ("[z-a]", "reversed"),
            ("[a/b]", "cannot contain"),
            ("{a,b", "never closed"),
            ("a}", "no matching"),
            ("{a,{b}", "never closed"),
            ("{a,b}{c,d}{e,f}{g,h}{i,j}{k,l}{m,n}", "more than"),
        ];
        for &(pattern, expected) in cases {
            let err = Glob::new(pattern, false).unwrap_err();
            assert!(
                err.message.contains(expected),
                "{pattern:?} -> {} (wanted {expected:?})",
                err.message
            );
            assert!(!err.fix.is_empty(), "{pattern:?} has no fix");
        }
    }

    #[test]
    fn specificity_ordering() {
        let spec = |p: &str| glob(p).alts()[0].specificity();
        let ordered = [
            "**",
            "**/*.md",
            "**/lib.rs",
            "src/**",
            "src/**/*.rs",
            "src/*.rs",
            "src/api/**",
            "src/lib.rs",
            "src/api/v1/lib.rs",
        ];
        for pair in ordered.windows(2) {
            assert!(
                spec(pair[0]) < spec(pair[1]),
                "{} should be less specific than {}",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn ban_patterns() {
        let ban = |p: &str, c: &str| BanPattern::new(p).is_match(c);
        assert!(ban("git push --force*", "git push --force"));
        assert!(ban(
            "git push --force*",
            "git push --force-with-lease origin main"
        ));
        assert!(!ban("git push --force*", "git push origin main --force"));
        assert!(!ban("git push --force*", "echo git push --force"));
        assert!(ban("git reset --hard*", "git reset --hard HEAD~1"));
        assert!(ban("git reset --hard*", "git reset --hard"));
        assert!(!ban("git reset --hard", "git reset --hard HEAD~1"));
        assert!(ban("git reset --hard", "git reset --hard"));
        assert!(ban("git clean -f*", "git clean -fdx"));
        assert!(ban("rm -rf /*", "rm -rf /usr/local/bin"));
        assert!(ban("*--no-verify*", "git commit -m x --no-verify"));
        assert!(ban("git * --force", "git push origin main --force"));
        assert!(!ban("git * --force", "git --force"));
        // `?` and `[` are literal in ban patterns.
        assert!(ban("ls ?", "ls ?"));
        assert!(!ban("ls ?", "ls a"));
        assert!(ban("a[1]", "a[1]"));
        // Whitespace is normalized.
        assert!(ban("  git   push  --force* ", "git push --force"));
        assert!(!ban("git push", "GIT push"));
        assert!(BanPattern::new("*").matches_everything());
        assert!(BanPattern::new(" ** ").matches_everything());
        assert!(BanPattern::new("").matches_everything());
        assert!(!BanPattern::new("a*").matches_everything());
    }

    /// Cross-check the matcher against the `globset` crate (configured as in the spec) on
    /// patterns where the two agree by definition. The one deliberate difference, `a/**`
    /// matching `a`, is accounted for.
    #[test]
    fn agrees_with_globset() {
        let patterns = [
            "src/*.rs",
            "src/**",
            "**/x",
            "a/**/b",
            "**/.env*",
            ".git/**",
            "*.{rs,toml}",
            "{src,lib}/**/*.rs",
            "file[0-9].txt",
            "a?c",
            "*",
            "**",
            "docs/**/*.md",
            "a/b/c",
            "[ab]/*",
        ];
        let paths = [
            "a",
            "a/b",
            "a/b/c",
            "a/x/b",
            "a/x/y/b",
            "x",
            "q/x",
            "q/r/x",
            "src",
            "src/lib.rs",
            "src/a/lib.rs",
            "src/a/b.toml",
            "lib/a/b.rs",
            ".env",
            "app/.env.local",
            ".git",
            ".git/config",
            ".github/x",
            "main.rs",
            "Cargo.toml",
            "file1.txt",
            "filex.txt",
            "abc",
            "ac",
            "a/c",
            "docs/a.md",
            "docs/x/y/a.md",
            "docs",
            "b/z",
        ];
        for pattern in patterns {
            let ours = glob(pattern);
            let theirs = globset::GlobBuilder::new(pattern)
                .literal_separator(true)
                .build()
                .unwrap()
                .compile_matcher();
            let prefix = pattern.strip_suffix("/**").map(|p| {
                globset::GlobBuilder::new(p)
                    .literal_separator(true)
                    .build()
                    .unwrap()
                    .compile_matcher()
            });
            for path in paths {
                let expected = theirs.is_match(path)
                    || prefix.as_ref().is_some_and(|prefix| prefix.is_match(path));
                assert_eq!(ours.is_match(path), expected, "{pattern:?} vs {path:?}");
            }
        }
    }
}
