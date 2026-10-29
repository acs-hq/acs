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
