//! Helpers shared by the commands of WP2.5: output flags, errors, text fitting (80 columns),
//! durations and `PATH` lookup.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use acs_core::{Feature, Timestamp};
use acs_store::SeatInfo;
use clap::Args;
use serde::Serialize;
use serde_json::json;

use crate::runtime::seats::current_human_seat;
use crate::runtime::{OpError, OpenError, RepoContext};

/// The width human output is kept within.
pub const WIDTH: usize = 80;

/// The output flag every command takes.
#[derive(Debug, Clone, Copy, Args)]
pub struct OutputArgs {
    /// Print one machine-readable JSON document on stdout instead of text.
    #[arg(long)]
    pub json: bool,
}

/// A command failure: a stable `code` for `--json` and a human message.
#[derive(Debug)]
pub struct CliError {
    /// Short machine-readable identifier (`not_a_repo`, `state_unavailable`, ...).
    pub code: &'static str,
    /// What to tell the person, including what to do next.
    pub message: String,
}

impl CliError {
    /// An error with `code` and `message`.
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl From<OpenError> for CliError {
    fn from(err: OpenError) -> Self {
        let code = match err {
            OpenError::NotARepo(_) => "not_a_repo",
            OpenError::Paths(_) => "paths",
        };
        Self::new(code, err.to_string())
    }
}

impl From<OpError> for CliError {
    fn from(err: OpError) -> Self {
        let code = match err {
            OpError::Unavailable(_) => "state_unavailable",
            OpError::Store(_) => "store",
            OpError::Invalid(_) => "invalid",
        };
        let hint = match err {
            OpError::Unavailable(_) | OpError::Store(_) => " (run `acs doctor`)",
            OpError::Invalid(_) => "",
        };
        Self::new(code, format!("{err}{hint}"))
    }
}

/// Turns a command's result into its exit code, reporting an error on stderr, or as a JSON
/// object on stdout with `--json` (one document, nothing on stderr).
#[must_use]
pub fn finish(json: bool, result: Result<ExitCode, CliError>) -> ExitCode {
    match result {
        Ok(code) => code,
        Err(err) => {
            if json {
                print_json(&json!({
                    "ok": false,
                    "error": {"code": err.code, "message": err.message},
                }));
            } else {
                eprintln!("acs: {}", err.message);
            }
            ExitCode::from(1)
        }
    }
}

/// Prints `value` as one pretty JSON document.
pub fn print_json(value: &impl Serialize) {
    match serde_json::to_string_pretty(value) {
        Ok(text) => println!("{text}"),
        Err(err) => {
            println!("{{\"ok\":false,\"error\":{{\"code\":\"json\",\"message\":\"{err}\"}}}}")
        }
    }
}

/// Opens the repository around the current directory.
///
/// # Errors
///
/// `not_a_repo` outside a git work tree.
pub fn open_repo() -> Result<(RepoContext, PathBuf), CliError> {
    let cwd = std::env::current_dir()
        .map_err(|err| CliError::new("cwd", format!("cannot read the current directory: {err}")))?;
    let ctx = RepoContext::open(&cwd)?;
    Ok((ctx, cwd))
}

/// Tells a person (on stderr, and not with `--json`) that `feature` is switched off in this
/// repository, for commands that still show what was recorded before. `effect` finishes the
/// sentence "... is switched off here, so".
pub fn note_feature_off(ctx: &RepoContext, feature: Feature, json: bool, effect: &str) {
    if json || ctx.feature_on(feature) {
        return;
    }
    eprint!(
        "{}",
        wrap(
            &format!(
                "Note: {feature} is switched off here, so {effect} (`acs features {feature} on` \
                 switches it back on)."
            ),
            "",
            "  ",
        )
    );
}

/// The human seat of the repository's pilot, created on first use.
///
/// # Errors
///
/// `state_unavailable` when the state database cannot be used.
pub fn human_seat(ctx: &RepoContext, now: Timestamp) -> Result<SeatInfo, CliError> {
    let db = ctx
        .db()
        .map_err(|why| CliError::from(OpError::Unavailable(why.to_owned())))?;
    Ok(current_human_seat(ctx, db, now)
        .map_err(OpError::from)?
        .info)
}

// ---------------------------------------------------------------------------------------------
// Text fitting
// ---------------------------------------------------------------------------------------------

/// `text` cut to at most `max` characters, keeping the **end** (paths: the file name matters
/// most) and marking the cut with `...`.
#[must_use]
pub fn ellipsize_start(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_owned();
    }
    if max <= 3 {
        return text.chars().skip(count - max).collect();
    }
    let tail: String = text.chars().skip(count - (max - 3)).collect();
    format!("...{tail}")
}

/// `text` cut to at most `max` characters, keeping the **start**.
#[must_use]
pub fn ellipsize_end(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_owned();
    }
    if max <= 3 {
        return text.chars().take(max).collect();
    }
    let head: String = text.chars().take(max - 3).collect();
    format!("{head}...")
}

/// Wraps `text` (which may hold several lines) to [`WIDTH`] columns. The first line of each
/// paragraph starts with `first`, the rest with `rest`. Words longer than the line are cut.
#[must_use]
pub fn wrap(text: &str, first: &str, rest: &str) -> String {
    let mut out = String::new();
    for paragraph in text.lines() {
        let mut line = first_or_rest(&out, first, rest);
        let mut has_word = false;
        for word in paragraph.split_whitespace() {
            let room = WIDTH.saturating_sub(line.chars().count());
            let word = if word.chars().count() > WIDTH - rest.chars().count() {
                ellipsize_end(word, WIDTH - rest.chars().count())
            } else {
                word.to_owned()
            };
            let needed = word.chars().count() + usize::from(has_word);
            if has_word && needed > room {
                out.push_str(line.trim_end());
                out.push('\n');
                line = rest.to_owned();
                has_word = false;
            }
            if has_word {
                line.push(' ');
            }
            line.push_str(&word);
            has_word = true;
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

fn first_or_rest(out: &str, first: &str, rest: &str) -> String {
    if out.is_empty() { first } else { rest }.to_owned()
}

/// One row of a two-column listing: `left` (cut from the start if needed) padded, then `right`
/// (cut from the end if needed), indented by two spaces, at most [`WIDTH`] wide.
#[must_use]
pub fn row(left: &str, right: &str) -> String {
    let right = ellipsize_end(right, 46);
    let right_width = right.chars().count();
    let budget = WIDTH.saturating_sub(2 + 2 + right_width).max(12);
    let left = ellipsize_start(left, budget);
    format!("  {left:<budget$}  {right}")
}

// ---------------------------------------------------------------------------------------------
// Durations
// ---------------------------------------------------------------------------------------------

/// A duration in seconds as `45s`, `3m`, `2h` or `4d`.
#[must_use]
pub fn short_duration(secs: i64) -> String {
    let secs = secs.max(0);
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m", secs / 60),
        3600..=86399 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86400),
    }
}

/// How long before `now` the timestamp `at` was.
#[must_use]
pub fn age(now: Timestamp, at: Timestamp) -> String {
    short_duration(now.as_second().saturating_sub(at.as_second()))
}

// ---------------------------------------------------------------------------------------------
// PATH
// ---------------------------------------------------------------------------------------------

/// The first executable called `name` on `PATH` (with the `PATHEXT` extensions on Windows).
#[must_use]
pub fn find_on_path(name: &str) -> Option<PathBuf> {
    let extensions: Vec<String> = if cfg!(windows) {
        let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".EXE;.CMD;.BAT;.COM".to_owned());
        std::iter::once(String::new())
            .chain(
                pathext
                    .split(';')
                    .filter(|e| !e.is_empty())
                    .map(str::to_owned),
            )
            .collect()
    } else {
        vec![String::new()]
    };
    search_path(name, &extensions)
}

/// Like [`find_on_path`], for a program this process will start itself. On Windows only the
/// extensions `CreateProcess` can run count: npm puts a POSIX shim called `claude` next to
/// `claude.cmd`, and [`find_on_path`] would return the shim.
#[must_use]
pub fn find_spawnable_on_path(name: &str) -> Option<PathBuf> {
    if cfg!(windows) {
        let extensions = [".exe", ".cmd", ".bat", ".com"].map(str::to_owned);
        search_path(name, &extensions)
    } else {
        find_on_path(name)
    }
}

fn search_path(name: &str, extensions: &[String]) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        for ext in extensions {
            let candidate = dir.join(format!("{name}{ext}"));
            if is_executable_file(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ellipsize_keeps_the_end_or_the_start() {
        assert_eq!(ellipsize_start("short", 10), "short");
        assert_eq!(
            ellipsize_start("src/payments/ledger.rs", 12),
            "...ledger.rs"
        );
        assert_eq!(ellipsize_end("alice@example.com", 10), "alice@e...");
        assert_eq!(ellipsize_start("abcdef", 2), "ef");
    }

    #[test]
    fn wrap_stays_within_the_width_and_indents() {
        let text = "word ".repeat(60);
        let wrapped = wrap(&text, "  ", "    ");
        assert!(wrapped.lines().count() > 2);
        for line in wrapped.lines() {
            assert!(line.chars().count() <= WIDTH, "{line}");
        }
        assert!(wrapped.lines().next().is_some_and(|l| l.starts_with("  w")));
        assert!(
            wrapped
                .lines()
                .nth(1)
                .is_some_and(|l| l.starts_with("    w"))
        );
        let long = wrap(&"x".repeat(200), "", "  ");
        assert!(long.lines().all(|l| l.chars().count() <= WIDTH));
    }

    #[test]
    fn rows_fit_and_keep_the_right_column() {
        let r = row(&"a/".repeat(80), "claude-code sara@example.com 3m");
        assert!(r.chars().count() <= WIDTH, "{r}");
        assert!(r.ends_with("3m"));
    }

    #[test]
    fn durations_are_short() {
        assert_eq!(short_duration(5), "5s");
        assert_eq!(short_duration(125), "2m");
        assert_eq!(short_duration(7300), "2h");
        assert_eq!(short_duration(200_000), "2d");
        assert_eq!(short_duration(-4), "0s");
    }
}
