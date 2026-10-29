//! A compact, stable rendering of a `ShellAnalysis`, used by the corpus test and the
//! `analyze` example.
//!
//! One command renders as:
//!
//! - `w`: the words joined by spaces (words with spaces, quotes or backslashes are shown with
//!   Rust debug quoting),
//! - `c`: the classes joined by commas,
//! - `t`: the targets, each `<read|write|delete>[+rec][+glob|+partial] <path>`,
//! - `u`: present (and `true`) when the targets are not known to be complete,
//! - `g`: for git, `<subcommand>` followed by `+rewrite`, `+force`, `+commit` and `refs=a,b`.
#![allow(dead_code)]

use acs_core::gate::{Confidence, ShellAnalysis, SimpleCommand, TargetOp};
use acs_shell::Dialect;
use serde::{Deserialize, Serialize};

/// One rendered command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rendered {
    pub w: String,
    pub c: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub t: Vec<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub u: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub g: String,
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn word(w: &str) -> String {
    if w.is_empty()
        || w.chars()
            .any(|c| c.is_whitespace() || c == '"' || c == '\\' || c.is_control())
    {
        format!("{w:?}")
    } else {
        w.to_string()
    }
}

fn class_name(class: acs_core::gate::CommandClass) -> String {
    serde_json::to_value(class)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

pub fn render_command(command: &SimpleCommand) -> Rendered {
    let targets = command
        .targets
        .iter()
        .map(|t| {
            let op = match t.op {
                TargetOp::Read => "read",
                TargetOp::Write => "write",
                TargetOp::Delete => "delete",
            };
            let rec = if t.recursive { "+rec" } else { "" };
            let conf = match t.confidence {
                Confidence::Exact => "",
                Confidence::Pattern => "+glob",
                Confidence::Partial => "+partial",
            };
            format!("{op}{rec}{conf} {}", t.path)
        })
        .collect();
    let git = command
        .git
        .as_ref()
        .map(|g| {
            let mut parts = vec![g.subcommand.clone()];
            if g.rewrites_history {
                parts.push("+rewrite".into());
            }
            if g.force_push {
                parts.push("+force".into());
            }
            if g.creates_commit {
                parts.push("+commit".into());
            }
            if !g.refs.is_empty() {
                parts.push(format!("refs={}", g.refs.join(",")));
            }
            parts.join(" ")
        })
        .unwrap_or_default();
    Rendered {
        w: command
            .words
            .iter()
            .map(|w| word(w))
            .collect::<Vec<_>>()
            .join(" "),
        c: command
            .classes
            .iter()
            .map(|c| class_name(*c))
            .collect::<Vec<_>>()
            .join(","),
        t: targets,
        u: !command.targets_known,
        g: git,
    }
}

pub fn render(analysis: &ShellAnalysis) -> Vec<Rendered> {
    analysis.commands.iter().map(render_command).collect()
}

// ---------------------------------------------------------------------------------------------
// The corpus file
// ---------------------------------------------------------------------------------------------

/// The corpus of command lines with their expected analysis.
pub const CORPUS: &str = include_str!("../fixtures/corpus.txt");

/// One case of the corpus.
pub struct Case {
    pub command: String,
    pub dialect: Dialect,
    /// Line number of the `$` line, for messages.
    pub line: usize,
    pub expected: Vec<Rendered>,
    pub complete: bool,
    /// Whether any expectation line was present.
    pub blessed: bool,
}

/// Whether a corpus line starts a case.
pub fn is_case_start(line: &str) -> bool {
    line.starts_with("$ps ") || line.starts_with("$cmd ") || line.starts_with("$ ") || line == "$"
}

/// Parses the corpus file (see its header for the format).
pub fn parse_corpus(text: &str) -> Result<Vec<Case>, String> {
    let mut cases: Vec<Case> = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let number = index + 1;
        let start = |dialect: Dialect, rest: &str| Case {
            command: rest.to_string(),
            dialect,
            line: number,
            expected: Vec::new(),
            complete: true,
            blessed: false,
        };
        if let Some(rest) = line.strip_prefix("$ps ") {
            cases.push(start(Dialect::PowerShell, rest));
        } else if let Some(rest) = line.strip_prefix("$cmd ") {
            cases.push(start(Dialect::Cmd, rest));
        } else if let Some(rest) = line.strip_prefix("$ ") {
            cases.push(start(Dialect::Bash, rest));
        } else if line == "$" {
            cases.push(start(Dialect::Bash, ""));
        } else if let Some(rest) = line.strip_prefix("| ") {
            let case = cases
                .last_mut()
                .ok_or(format!("line {number}: continuation without a case"))?;
            case.command.push('\n');
            case.command.push_str(rest);
        } else if let Some(rest) = line.strip_prefix("> ") {
            let case = cases
                .last_mut()
                .ok_or(format!("line {number}: expectation without a case"))?;
            case.blessed = true;
            if rest == "!incomplete" {
                case.complete = false;
            } else if rest == "(none)" {
                // No commands at all.
            } else {
                let rendered: Rendered = serde_json::from_str(rest)
                    .map_err(|e| format!("line {number}: bad expectation: {e}"))?;
                case.expected.push(rendered);
            }
        } else if !(line.is_empty() || line.starts_with('#')) {
            return Err(format!("line {number}: cannot parse {line:?}"));
        }
    }
    Ok(cases)
}
