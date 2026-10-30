//! `acs init`: set a repository up for ACS.
//!
//! 1. Writes `.acs/policy/` defaults ([`policy`]); existing files are never touched.
//! 2. Adds the ACS fetch refspec to every configured remote ([`remotes`]).
//! 3. Installs the git hooks ([`hooks`], WP2.4's installer behind one call).
//! 4. Detects harnesses and prints the commands that install their plugin ([`harness`]); it does
//!    not run them.
//! 5. Checks that `acs` resolves on `PATH` and warns loudly if it does not.
//!
//! Running it again changes nothing. There are no prompts, so there is no `--yes` yet (WP5.3's
//! proposals add the first prompt and the flag).

pub mod harness;
pub mod hooks;
pub mod policy;
pub mod remotes;

use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;

use acs_paths::PathResolver;
use acs_policy::{LoadOptions, load};
use clap::Args;
use serde::Serialize;

use self::harness::{Detected, detect};
use self::hooks::{HooksInstall, install as install_hooks};
use self::policy::{FileAction, PolicyFileReport, write_missing};
use self::remotes::{RemoteAction, RemoteReport};
use super::support::{
    CliError, OutputArgs, ellipsize_start, find_on_path, finish, print_json, wrap,
};

/// Arguments of `acs init`.
#[derive(Debug, Args)]
#[command(
    after_help = "Safe to run again: existing policy files are never overwritten and nothing \
                  that is already set up changes."
)]
pub struct InitArgs {
    /// Output format.
    #[command(flatten)]
    pub output: OutputArgs,
}

/// Whether the `acs` binary resolves on `PATH`.
#[derive(Debug, Clone, Serialize)]
pub struct AcsOnPath {
    /// Found on `PATH`.
    pub found: bool,
    /// Where.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// Everything `acs init` did and found; also the `--json` document.
#[derive(Debug, Clone, Serialize)]
pub struct InitReport {
    /// False when a step failed.
    pub ok: bool,
    /// Whether anything on disk or in git config changed.
    pub changed: bool,
    /// The work tree.
    pub worktree: String,
    /// The policy directory, relative to the work tree.
    pub policy_dir: &'static str,
    /// One entry per policy file.
    pub policy_files: Vec<PolicyFileReport>,
    /// One entry per configured remote.
    pub remotes: Vec<RemoteReport>,
    /// The git hook step.
    pub git_hooks: HooksInstall,
    /// Harnesses found, with the install commands.
    pub harnesses: Vec<Detected>,
    /// The `PATH` check for `acs`.
    pub acs_on_path: AcsOnPath,
    /// Things that need attention but did not stop `init`.
    pub warnings: Vec<String>,
    /// Steps that failed.
    pub errors: Vec<String>,
}

/// Runs `acs init`.
#[must_use]
pub fn run(args: &InitArgs) -> ExitCode {
    finish(args.output.json, execute(args))
}

fn execute(args: &InitArgs) -> Result<ExitCode, CliError> {
    let cwd = std::env::current_dir()
        .map_err(|err| CliError::new("cwd", format!("cannot read the current directory: {err}")))?;
    let layout =
        acs_git::discover(&cwd).map_err(|err| CliError::new("not_a_repo", err.to_string()))?;
    let report = initialize(&layout);
    let report = report.map_err(|err| CliError::new("init_failed", err))?;
    if args.output.json {
        print_json(&report);
    } else {
        print!("{}", render(&report));
    }
    Ok(if report.ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

/// Performs the steps against `layout`'s repository.
///
/// # Errors
///
/// A message when the policy directory cannot be written (nothing else is attempted then).
pub fn initialize(layout: &acs_git::RepoLayout) -> Result<InitReport, String> {
    let work_tree = &layout.work_tree;
    let policy_dir = work_tree.join(".acs").join("policy");
    let policy_files = write_missing(&policy_dir)
        .map_err(|err| format!("cannot write {}: {err}", policy_dir.display()))?;
    let mut warnings = Vec::new();
    let mut errors = Vec::new();

    // Validate what is on disk now (ours or the team's); a broken policy is reported, not fatal.
    let case_insensitive = PathResolver::new(work_tree).is_ok_and(|r| r.case_insensitive());
    match load(&policy_dir, LoadOptions { case_insensitive }) {
        Ok(loaded) => warnings.extend(loaded.warnings),
        Err(err) => warnings.push(format!(
            "the policy in .acs/policy is invalid and will not be enforced until fixed: {err}"
        )),
    }

    let mut remote_reports = Vec::new();
    match remotes::list(work_tree) {
        Ok(names) => {
            for name in names {
                match remotes::ensure(work_tree, &name) {
                    Ok(report) => remote_reports.push(report),
                    Err(err) => errors.push(format!("remote `{name}`: {err}")),
                }
            }
        }
        Err(err) => errors.push(err),
    }

    let git_hooks = install_hooks(layout);
    match &git_hooks {
        HooksInstall::Done { .. } => {}
        HooksInstall::Pending { note } => warnings.push(note.clone()),
        HooksInstall::Failed { error } => errors.push(format!("git hooks: {error}")),
    }

    let harnesses = detect(work_tree);
    let acs = find_on_path("acs");
    if acs.is_none() {
        let mut text = "`acs` is not on PATH. The harness hooks and git hooks run `acs`, so \
                        nothing is enforced until it is installed on PATH."
            .to_owned();
        for found in harnesses.iter().filter(|h| h.fails_closed_without_acs) {
            let _ = write!(
                text,
                " With the {} plugin installed, every tool call would be DENIED.",
                found.name
            );
        }
        warnings.push(text);
    }

    let changed = policy_files.iter().any(|f| f.action == FileAction::Created)
        || remote_reports
            .iter()
            .any(|r| r.action == RemoteAction::Added)
        || matches!(&git_hooks, HooksInstall::Done { hooks } if hooks.iter().any(|h| h.action != "unchanged"));
    Ok(InitReport {
        ok: errors.is_empty(),
        changed,
        worktree: work_tree.to_string_lossy().into_owned(),
        policy_dir: ".acs/policy",
        policy_files,
        remotes: remote_reports,
        git_hooks,
        harnesses,
        acs_on_path: AcsOnPath {
            found: acs.is_some(),
            path: acs.map(|p: PathBuf| p.to_string_lossy().into_owned()),
        },
        warnings,
        errors,
    })
}

/// The human view, at most 80 columns wide.
#[must_use]
pub fn render(report: &InitReport) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "acs init: {}",
        ellipsize_start(&report.worktree, 80 - "acs init: ".len())
    );

    let _ = writeln!(out, "\nPolicy ({}/)", report.policy_dir);
    for file in &report.policy_files {
        let word = match file.action {
            FileAction::Created => "created",
            FileAction::Exists => "kept",
        };
        let _ = writeln!(out, "  {word:<8}{}", file.file);
    }

    out.push_str("\nFetch refspec for claims shared through git\n");
    if report.remotes.is_empty() {
        out.push_str(&wrap(
            "none: no git remote is configured. Claims stay on this machine; run `acs init` \
             again after adding a remote.",
            "  ",
            "  ",
        ));
    }
    for remote in &report.remotes {
        let word = match remote.action {
            RemoteAction::Added => "added",
            RemoteAction::Present => "present",
        };
        out.push_str(&wrap(
            &format!("{word}: {} {}", remote.name, remote.refspec),
            "  ",
            "    ",
        ));
    }

    out.push_str("\nGit hooks\n");
    match &report.git_hooks {
        HooksInstall::Done { hooks } => {
            for hook in hooks {
                let _ = writeln!(out, "  {:<10}{}", hook.action, hook.name);
            }
        }
        HooksInstall::Pending { note } => out.push_str(&wrap(note, "  ", "  ")),
        HooksInstall::Failed { error } => {
            out.push_str(&wrap(&format!("failed: {error}"), "  ", "  "))
        }
    }

    out.push_str("\nAgent harnesses\n");
    if report.harnesses.is_empty() {
        out.push_str("  none detected (looked for Claude Code and Antigravity)\n");
    }
    for found in &report.harnesses {
        out.push_str(&wrap(
            &format!("{} detected ({}).", found.name, found.found.join(", ")),
            "  ",
            "  ",
        ));
        out.push_str("  To install the ACS plugin, run:\n");
        for command in &found.install {
            out.push_str(&wrap(command, "    ", "      "));
        }
    }

    for warning in &report.warnings {
        out.push('\n');
        out.push_str(&wrap(&format!("WARNING: {warning}"), "!! ", "   "));
    }
    for error in &report.errors {
        out.push('\n');
        out.push_str(&wrap(&format!("ERROR: {error}"), "!! ", "   "));
    }

    if report.ok && !report.changed {
        out.push_str("\nNothing to change: this repository is already set up.\n");
    } else if report.ok {
        out.push_str(
            "\nCommit .acs/policy/ so every clone shares it. `acs status` shows the state.\n",
        );
    }
    out
}
