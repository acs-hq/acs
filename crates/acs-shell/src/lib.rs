//! ACS shell: static analysis of shell command lines.
//!
//! [`analyze`] turns the text of a shell command into a [`ShellAnalysis`]: every simple command
//! in it (across pipes, lists, subshells, command substitutions, control structures and the
//! scripts of `sh -c`/`eval`), what each one is classified as, and the paths it acts on, with
//! a [`Confidence`](acs_core::gate::Confidence) for each path.
//!
//! Bash is parsed with tree-sitter. PowerShell and cmd.exe are handled by a small tokenizer and
//! heuristics for the commands that delete, move and write files; anything else is unknown.
//!
//! The analysis is conservative: whatever cannot be seen (scripts, interpreters, `eval`, unknown
//! programs, expansions) is reported as unknown, never as harmless. It performs no I/O.
//! Decisions on top of the analysis belong to the decision engine, not to this crate.
//!
//! Conventions worth knowing when consuming a [`ShellAnalysis`]:
//!
//! - `words` of a command are the effective command: leading `NAME=value` assignments and
//!   wrappers (`sudo`, `env`, `nice`, `time`, `command`, `builtin`, `exec`, `nohup`, `timeout`,
//!   ...) are removed, so `sudo rm -rf x` has the words `rm -rf x`.
//! - Target paths are joined with the directory of a preceding literal `cd`; after a `cd`
//!   whose directory is unknown, relative targets are `Partial`.
//! - `mv`, `cp`, `ln` and `install` mark their destination (and `mv` its sources) recursive,
//!   because a directory may be involved and sources may land inside it.
//! - A target that begins with `~` is `Partial`: the home directory is not known here.

mod args;
mod bash;
mod ctx;
mod git;
mod table;
mod tools;
mod windows;
mod word;

use acs_core::gate::ShellAnalysis;
use serde::{Deserialize, Serialize};

/// The shell language of a command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dialect {
    /// POSIX-style shells (bash, sh, zsh), parsed exactly.
    #[default]
    Bash,
    /// PowerShell, understood heuristically.
    #[serde(rename = "powershell")]
    PowerShell,
    /// cmd.exe, understood heuristically.
    Cmd,
}

/// Analyzes a shell command line.
///
/// Creates its parser per call and keeps no global state, so it is safe to call from any
/// thread. A line that does not parse still yields the commands that could be recognized, with
/// [`ShellAnalysis::complete`] cleared.
#[must_use]
pub fn analyze(command: &str, dialect: Dialect) -> ShellAnalysis {
    let mut ctx = ctx::Ctx::new();
    match dialect {
        Dialect::Bash => bash::run(command, &mut ctx),
        Dialect::PowerShell => windows::powershell(command, &mut ctx),
        Dialect::Cmd => windows::cmd(command, &mut ctx),
    }
    ShellAnalysis {
        commands: ctx.out,
        complete: ctx.complete,
    }
}
