//! The clap command tree and dispatch.
//!
//! Later work packages add one variant to [`Command`] and one arm to [`run`] each.

use std::io::{Read, Write};
use std::process::ExitCode;

use acs_core::Phase;
use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::commands::git_hook::{self, GitHookArgs};
use crate::hook::{self, HookEnv};

/// The `acs` command line.
#[derive(Debug, Parser)]
#[command(
    name = "acs",
    version,
    about = "Agent Control System: coordinate coding agents in one repository"
)]
pub struct Cli {
    /// What to do.
    #[command(subcommand)]
    pub command: Command,
}

/// The commands. Registration point: one variant per command, in the order of decision 0011.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Handle one harness hook call: read the payload on stdin, write the decision on stdout.
    Hook(HookArgs),
    /// Run by the git hooks `acs init` installs (pre-commit, prepare-commit-msg, pre-push).
    #[command(name = "git-hook")]
    GitHook(GitHookArgs),
}

/// A hook phase on the command line (the names of the spec's `phase` field).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum PhaseArg {
    /// Before a tool call.
    #[value(name = "pre_tool", alias = "pre-tool")]
    PreTool,
    /// After a tool call.
    #[value(name = "post_tool", alias = "post-tool")]
    PostTool,
    /// A session started.
    #[value(name = "session_start", alias = "session-start")]
    SessionStart,
    /// The user submitted a prompt.
    #[value(name = "prompt_submit", alias = "prompt-submit")]
    PromptSubmit,
    /// A session ended.
    #[value(name = "session_end", alias = "session-end")]
    SessionEnd,
}

impl From<PhaseArg> for Phase {
    fn from(arg: PhaseArg) -> Self {
        match arg {
            PhaseArg::PreTool => Self::PreTool,
            PhaseArg::PostTool => Self::PostTool,
            PhaseArg::SessionStart => Self::SessionStart,
            PhaseArg::PromptSubmit => Self::PromptSubmit,
            PhaseArg::SessionEnd => Self::SessionEnd,
        }
    }
}

/// Arguments of `acs hook`.
#[derive(Debug, Args)]
#[command(
    after_help = "Examples:\n  acs hook --canonical pre_tool < event.json\n  acs hook claude-code --phase pre_tool < payload.json"
)]
pub struct HookArgs {
    /// Speak the canonical protocol: stdin is a ToolEvent, stdout a Decision, exit code 0.
    #[arg(long, value_name = "PHASE", conflicts_with_all = ["harness", "phase"])]
    pub canonical: Option<PhaseArg>,
    /// The harness whose adapter parses stdin and renders the decision.
    #[arg(value_name = "HARNESS", required_unless_present = "canonical")]
    pub harness: Option<String>,
    /// The phase, for harnesses that run a different command per phase.
    #[arg(long, value_name = "PHASE")]
    pub phase: Option<PhaseArg>,
}

/// Parses the process's arguments and runs the command.
#[must_use]
pub fn run_from_env() -> ExitCode {
    match Cli::try_parse() {
        Ok(cli) => run(cli),
        Err(err) => {
            let usage_error = err.use_stderr();
            let _ = err.print();
            if !usage_error {
                ExitCode::SUCCESS
            } else if std::env::args().nth(1).as_deref() == Some("git-hook") {
                // For a git hook any non-zero exit rejects the commit or push, and the backstop
                // fails open (decision 0015): a misuse prints the usage and lets git proceed.
                ExitCode::SUCCESS
            } else if std::env::args().nth(1).as_deref() == Some("hook") {
                // clap's usual exit code 2 means "block the tool call" to some harnesses; a
                // misconfigured hook must not do that.
                ExitCode::from(1)
            } else {
                ExitCode::from(2)
            }
        }
    }
}

/// Runs a parsed command.
#[must_use]
pub fn run(cli: Cli) -> ExitCode {
    match cli.command {
        Command::Hook(args) => run_hook(&args),
        Command::GitHook(args) => git_hook::run(&args),
    }
}

fn run_hook(args: &HookArgs) -> ExitCode {
    let (name, phase) = match (&args.canonical, &args.harness) {
        (Some(phase), _) => ("canonical", Some(Phase::from(*phase))),
        (None, Some(harness)) => (harness.as_str(), args.phase.map(Phase::from)),
        (None, None) => ("canonical", None),
    };
    let Some(adapter) = acs_adapters::by_name(name) else {
        eprintln!(
            "acs: unknown harness `{name}`; known: {}",
            acs_adapters::names().join(", ")
        );
        return ExitCode::from(1);
    };
    let mut payload = Vec::new();
    // A stdin we cannot read is treated as an empty payload: the adapter reports it and the
    // pipeline allows with `unknown_payload`.
    let _ = std::io::stdin().read_to_end(&mut payload);
    let output = hook::run(adapter, phase, &payload, HookEnv::from_process());
    let _ = std::io::stdout().write_all(&output.stdout);
    let _ = std::io::stdout().flush();
    if !output.stderr.is_empty() {
        let _ = std::io::stderr().write_all(&output.stderr);
    }
    ExitCode::from(u8::try_from(output.exit_code).unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("acs").chain(args.iter().copied()))
    }

    #[test]
    fn canonical_takes_a_phase() {
        let cli = parse(&["hook", "--canonical", "pre_tool"]).unwrap();
        let Command::Hook(args) = cli.command else {
            panic!("not a hook command")
        };
        assert_eq!(args.canonical, Some(PhaseArg::PreTool));
        assert!(args.harness.is_none());
        assert!(parse(&["hook", "--canonical", "pre-tool"]).is_ok());
    }

    #[test]
    fn harness_form_takes_an_optional_phase() {
        let cli = parse(&["hook", "claude-code", "--phase", "session_end"]).unwrap();
        let Command::Hook(args) = cli.command else {
            panic!("not a hook command")
        };
        assert_eq!(args.harness.as_deref(), Some("claude-code"));
        assert_eq!(args.phase, Some(PhaseArg::SessionEnd));
        assert!(parse(&["hook", "claude-code"]).is_ok());
    }

    #[test]
    fn a_hook_needs_canonical_or_a_harness_but_not_both() {
        assert!(parse(&["hook"]).is_err());
        assert!(parse(&["hook", "--canonical", "pre_tool", "claude-code"]).is_err());
        assert!(parse(&["hook", "--canonical", "teleport"]).is_err());
    }

    #[test]
    fn phase_names_map_to_the_spec_phases() {
        assert_eq!(Phase::from(PhaseArg::PromptSubmit), Phase::PromptSubmit);
        assert_eq!(Phase::from(PhaseArg::SessionStart), Phase::SessionStart);
    }

    #[test]
    fn git_hook_passes_git_arguments_through() {
        let cli = parse(&[
            "git-hook",
            "prepare-commit-msg",
            ".git/COMMIT_EDITMSG",
            "message",
        ])
        .unwrap();
        let Command::GitHook(args) = cli.command else {
            panic!("not a git-hook command")
        };
        assert_eq!(args.name, "prepare-commit-msg");
        assert_eq!(args.args, [".git/COMMIT_EDITMSG", "message"]);
        let cli = parse(&["git-hook", "pre-push", "origin", "-weird-url"]).unwrap();
        let Command::GitHook(args) = cli.command else {
            panic!("not a git-hook command")
        };
        assert_eq!(args.args, ["origin", "-weird-url"]);
    }
}
