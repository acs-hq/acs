//! `acs init`: set a repository up for ACS.
//!
//! 1. Writes `.acs/policy/` defaults ([`policy`]); existing files are never touched.
//! 2. Adds the ACS fetch refspec to every configured remote ([`remotes`]).
//! 3. Installs the git hooks ([`hooks`], WP2.4's installer behind one call).
//! 4. Detects harnesses and prints the commands that install their plugin ([`harness`]); it does
//!    not run them.
//! 5. Checks that `acs` resolves on `PATH` and warns loudly if it does not.
//!
//! Before step 1, while `territory.toml` is missing or still the untouched template, it proposes
//! zones from folder names and git history ([`propose`], decision 0025). The proposal is written
//! with `--yes`, or when the person answers yes at the one prompt (only in a terminal, only when
//! `territory.toml` is missing). Otherwise it is reported and the template is written.
//!
//! Running it again without `--yes` changes nothing.

pub mod branch;
pub mod harness;
pub mod hooks;
pub mod policy;
pub mod propose;
pub mod remotes;

use std::fmt::Write as _;
use std::io::{BufRead as _, IsTerminal as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use acs_core::gate::PolicyView as _;
use acs_core::{Feature, Features, ZoneKind};
use acs_paths::PathResolver;
use acs_policy::{LoadOptions, load};
use clap::Args;
use serde::Serialize;

use self::branch::DefaultBranch;
use self::harness::{Detected, detect};
use self::hooks::{HooksInstall, install as install_hooks};
use self::policy::{
    FileAction, PolicyFileReport, TERRITORY_HEADER, is_territory_template, write_missing,
};
use self::propose::{Proposal, Thresholds, propose_for, territory_toml};
use self::remotes::{RemoteAction, RemoteReport};
use super::support::{
    CliError, OutputArgs, ellipsize_start, find_on_path, finish, print_json, wrap,
};

/// Arguments of `acs init`.
#[derive(Debug, Args)]
#[command(
    after_help = "Safe to run again: existing policy files are never overwritten and nothing \
                  that is already set up changes. The one exception is `--yes`, which replaces a \
                  territory.toml that still holds the untouched default with the proposal."
)]
pub struct InitArgs {
    /// Write the proposed territory (zones from folder names and git history) without asking.
    #[arg(long, short = 'y')]
    pub yes: bool,

    /// Share of a folder's recent commits one author needs to be proposed as its owner, in
    /// percent.
    #[arg(long, value_name = "PERCENT", default_value_t = 75,
          value_parser = clap::value_parser!(u8).range(51..=100))]
    pub min_share: u8,

    /// Commits a folder needs in the last 12 months before anyone is proposed as its owner.
    #[arg(long, value_name = "N", default_value_t = 10,
          value_parser = clap::value_parser!(u32).range(1..))]
    pub min_commits: u32,

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
    /// The features switched off in `config.toml` (`sync` is, unless the repository opted in).
    pub features_off: Vec<Feature>,
    /// The repository's main branch and what the policy says about it.
    pub default_branch: BranchReport,
    /// One entry per configured remote; empty while team sync is off.
    pub remotes: Vec<RemoteReport>,
    /// The git hook step.
    pub git_hooks: HooksInstall,
    /// Harnesses found, with the install commands.
    pub harnesses: Vec<Detected>,
    /// The territory proposal and what became of it.
    pub territory: TerritoryReport,
    /// The `PATH` check for `acs`.
    pub acs_on_path: AcsOnPath,
    /// Things that need attention but did not stop `init`.
    pub warnings: Vec<String>,
    /// Steps that failed.
    pub errors: Vec<String>,
}

/// The main-branch step of the report (decision 0040).
#[derive(Debug, Clone, Serialize)]
pub struct BranchReport {
    /// The main branch found, if one was.
    pub found: Option<DefaultBranch>,
    /// The policy files this run wrote it into (none when it is `main` or the files existed).
    pub written_to: Vec<&'static str>,
    /// `default_target_branch` in the policy as it is now.
    pub target_branch: String,
    /// Whether the branch found matches `protected_branches` (true when none was found).
    pub protected: bool,
}

/// The warning for a policy that does not name the main branch found.
fn branch_mismatch(found: &DefaultBranch, report: &BranchReport) -> String {
    let mut problems = Vec::new();
    if found.name != report.target_branch {
        problems.push(format!(
            "config.toml has default_target_branch = \"{}\"",
            report.target_branch
        ));
    }
    if !report.protected {
        problems.push("guards.toml's protected_branches does not cover it".to_owned());
    }
    format!(
        "this repository's main branch looks like `{}` ({}), but {}. Edit .acs/policy/ if that \
         is wrong.",
        found.name,
        found.source.describe(),
        problems.join(", and ")
    )
}

/// What `init` did about `territory.toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TerritoryStatus {
    /// The proposal was written.
    Applied,
    /// Zones were proposed but not written (no `--yes`, or the person said no).
    Proposed,
    /// Nothing to propose; the whole repository stays `shared`.
    NothingProposed,
    /// `territory.toml` has been edited, so nothing was proposed and it was kept.
    Kept,
}

/// The territory step of the report.
#[derive(Debug, Clone, Serialize)]
pub struct TerritoryReport {
    /// What happened.
    pub status: TerritoryStatus,
    /// The proposal, unless `territory.toml` was kept or history could not be read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proposal: Option<Proposal>,
}

/// The state of `territory.toml` before `init`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerritoryFile {
    /// It does not exist.
    Missing,
    /// It holds the untouched template.
    Template,
    /// It has been edited (or cannot be read).
    Edited,
}

/// The territory proposal for a repository, before anything is written.
#[derive(Debug, Clone)]
pub struct TerritoryPlan {
    /// `territory.toml` as it is now.
    pub file: TerritoryFile,
    /// The proposal; `None` when the file was edited or history could not be read.
    pub proposal: Option<Proposal>,
    /// Why history could not be read.
    pub error: Option<String>,
}

impl TerritoryPlan {
    /// Whether there are zones to write.
    #[must_use]
    pub fn has_zones(&self) -> bool {
        self.proposal.as_ref().is_some_and(|p| !p.zones.is_empty())
    }
}

/// Looks at `territory.toml` and, unless it has been edited, proposes zones.
#[must_use]
pub fn plan_territory(work_tree: &Path, thresholds: &Thresholds) -> TerritoryPlan {
    let path = work_tree.join(".acs").join("policy").join("territory.toml");
    let file = match std::fs::read_to_string(&path) {
        Ok(text) if is_territory_template(&text) => TerritoryFile::Template,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => TerritoryFile::Missing,
        Ok(_) | Err(_) => TerritoryFile::Edited,
    };
    if file == TerritoryFile::Edited {
        return TerritoryPlan {
            file,
            proposal: None,
            error: None,
        };
    }
    match propose_for(work_tree, thresholds) {
        Ok(proposal) => TerritoryPlan {
            file,
            proposal: Some(proposal),
            error: None,
        },
        Err(err) => TerritoryPlan {
            file,
            proposal: None,
            error: Some(err),
        },
    }
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
    let thresholds = Thresholds {
        min_share_percent: args.min_share,
        min_commits: args.min_commits,
        ..Thresholds::default()
    };
    let plan = plan_territory(&layout.work_tree, &thresholds);
    let apply = args.yes
        || (!args.output.json
            && plan.file == TerritoryFile::Missing
            && plan.has_zones()
            && std::io::stdin().is_terminal()
            && std::io::stdout().is_terminal()
            && ask(&plan));
    let report = initialize(&layout, &plan, apply);
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

/// Shows the proposal and asks whether to write it. Anything but yes is no.
fn ask(plan: &TerritoryPlan) -> bool {
    let mut text = String::from("Proposed territory\n");
    render_zones(&mut text, plan.proposal.as_ref());
    print!("{text}");
    print!("\nWrite these zones to .acs/policy/territory.toml? [y/N] ");
    let _ = std::io::stdout().flush();
    let mut answer = String::new();
    if std::io::stdin().lock().read_line(&mut answer).is_err() {
        return false;
    }
    println!();
    matches!(answer.trim(), "y" | "Y" | "yes" | "Yes" | "YES")
}

/// Performs the steps against `layout`'s repository. `apply` writes `plan`'s zones.
///
/// # Errors
///
/// A message when the policy directory cannot be written (nothing else is attempted then).
pub fn initialize(
    layout: &acs_git::RepoLayout,
    plan: &TerritoryPlan,
    apply: bool,
) -> Result<InitReport, String> {
    let work_tree = &layout.work_tree;
    let policy_dir = work_tree.join(".acs").join("policy");
    let proposed = plan
        .proposal
        .as_ref()
        .filter(|p| apply && !p.zones.is_empty())
        .map(|p| territory_toml(TERRITORY_HEADER, p));
    let new_territory = proposed
        .as_deref()
        .filter(|_| plan.file == TerritoryFile::Missing);
    let detected_branch = branch::detect(work_tree);
    let mut policy_files = write_missing(
        &policy_dir,
        new_territory,
        detected_branch.as_ref().map(|b| b.name.as_str()),
    )
    .map_err(|err| format!("cannot write {}: {err}", policy_dir.display()))?;
    let mut warnings = Vec::new();
    let mut errors = Vec::new();

    let mut applied = new_territory.is_some()
        && policy_files
            .iter()
            .any(|f| f.file == "territory.toml" && f.action == FileAction::Created);
    if let Some(text) = proposed
        .as_deref()
        .filter(|_| plan.file == TerritoryFile::Template)
    {
        let path = policy_dir.join("territory.toml");
        // Re-check right before writing, so an edit made meanwhile is never lost.
        let untouched = std::fs::read_to_string(&path).is_ok_and(|t| is_territory_template(&t));
        if untouched {
            match std::fs::write(&path, text) {
                Ok(()) => {
                    applied = true;
                    for file in &mut policy_files {
                        if file.file == "territory.toml" {
                            file.action = FileAction::Replaced;
                        }
                    }
                }
                Err(err) => errors.push(format!("cannot write {}: {err}", path.display())),
            }
        }
    }
    if let Some(err) = &plan.error {
        warnings.push(format!(
            "could not read git history to propose territory, so the whole repository stays \
             shared: {err}"
        ));
    }
    let status = if plan.file == TerritoryFile::Edited {
        TerritoryStatus::Kept
    } else if applied {
        TerritoryStatus::Applied
    } else if plan.has_zones() {
        TerritoryStatus::Proposed
    } else {
        TerritoryStatus::NothingProposed
    };
    let territory = TerritoryReport {
        status,
        proposal: plan.proposal.clone(),
    };

    // Validate what is on disk now (ours or the team's); a broken policy is reported, not fatal.
    let case_insensitive = PathResolver::new(work_tree).is_ok_and(|r| r.case_insensitive());
    let mut features = Features::default();
    let mut default_branch = BranchReport {
        found: detected_branch.clone(),
        written_to: Vec::new(),
        target_branch: policy::DEFAULT_TARGET_BRANCH.to_owned(),
        protected: true,
    };
    if let Some(found) = &detected_branch
        && found.name != policy::DEFAULT_TARGET_BRANCH
    {
        default_branch.written_to = policy_files
            .iter()
            .filter(|f| f.action == FileAction::Created && f.file != "territory.toml")
            .map(|f| f.file)
            .collect();
    }
    match load(&policy_dir, LoadOptions { case_insensitive }) {
        Ok(loaded) => {
            features = loaded.policy.files().config.features;
            default_branch
                .target_branch
                .clone_from(&loaded.policy.files().config.default_target_branch);
            if let Some(found) = &detected_branch {
                default_branch.protected = loaded.policy.is_protected_branch(&found.name);
                if found.name != default_branch.target_branch || !default_branch.protected {
                    warnings.push(branch_mismatch(found, &default_branch));
                }
            }
            warnings.extend(loaded.warnings);
        }
        Err(err) => warnings.push(format!(
            "the policy in .acs/policy is invalid and will not be enforced until fixed: {err}"
        )),
    }

    // The fetch setting is only for repositories that switched team sync on: with it off,
    // nothing ACS does touches the remote or the git config for it.
    let mut remote_reports = Vec::new();
    if features.sync {
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

    let changed = policy_files.iter().any(|f| f.action != FileAction::Exists)
        || remote_reports
            .iter()
            .any(|r| r.action == RemoteAction::Added)
        || matches!(&git_hooks, HooksInstall::Done { hooks, .. } if hooks.iter().any(|h| h.action != "unchanged"));
    Ok(InitReport {
        ok: errors.is_empty(),
        changed,
        worktree: work_tree.to_string_lossy().into_owned(),
        policy_dir: ".acs/policy",
        policy_files,
        features_off: features.off(),
        default_branch,
        remotes: remote_reports,
        git_hooks,
        harnesses,
        territory,
        acs_on_path: AcsOnPath {
            found: acs.is_some(),
            path: acs.map(|p: PathBuf| p.to_string_lossy().into_owned()),
        },
        warnings,
        errors,
    })
}

/// The zones of `proposal`, one per line, then the note about `owned`.
fn render_zones(out: &mut String, proposal: Option<&Proposal>) {
    let Some(proposal) = proposal else { return };
    let mut any_owned = false;
    // Folders are padded to one column, but a very long one does not push the others out.
    let width = proposal
        .zones
        .iter()
        .map(|z| z.folder.chars().count() + 1)
        .filter(|w| *w <= 30)
        .max()
        .unwrap_or(0);
    for zone in &proposal.zones {
        let (kind, detail) = match zone.kind {
            ZoneKind::Owned => {
                any_owned = true;
                (
                    "owned",
                    format!("{}, {}", zone.owners.join(", "), zone.reason),
                )
            }
            ZoneKind::Open => ("open", zone.reason.clone()),
            ZoneKind::Shared => ("shared", zone.reason.clone()),
            ZoneKind::Sealed => ("sealed", zone.reason.clone()),
        };
        let folder = format!("{}/", zone.folder);
        let first = format!("    {kind:<6} {folder:<width$}  ");
        out.push_str(&wrap(&detail, &first, &" ".repeat(13 + width)));
    }
    if any_owned {
        out.push_str(&wrap(
            "Only the owners' agents may change owned zones; `acs override` lets others in.",
            "  ",
            "  ",
        ));
    }
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
            FileAction::Replaced => "replaced",
        };
        let _ = writeln!(out, "  {word:<9}{}", file.file);
    }

    out.push_str("\nTerritory (territory.toml)\n");
    let proposal = report.territory.proposal.as_ref();
    match report.territory.status {
        TerritoryStatus::Kept => {
            out.push_str("  kept: it has been edited, so nothing is proposed\n");
        }
        TerritoryStatus::Applied => {
            out.push_str("  written from folder names and git history:\n");
            render_zones(&mut out, proposal);
        }
        TerritoryStatus::Proposed => {
            out.push_str("  proposed from folder names and git history, not written:\n");
            render_zones(&mut out, proposal);
            out.push_str("  To write it, run `acs init --yes`, then edit the file as you like.\n");
        }
        TerritoryStatus::NothingProposed => {
            out.push_str("  nothing to propose: the whole repository is shared\n");
            for note in proposal.map(|p| p.notes.as_slice()).unwrap_or_default() {
                out.push_str(&wrap(note, "  ", "  "));
            }
        }
    }

    out.push_str("\nFeatures\n");
    let on: Vec<&str> = Feature::ALL
        .into_iter()
        .filter(|f| !report.features_off.contains(f))
        .map(Feature::key)
        .collect();
    let off: Vec<&str> = report.features_off.iter().map(|f| f.key()).collect();
    out.push_str(&wrap(&format!("on: {}", on.join(", ")), "  ", "    "));
    if !off.is_empty() {
        out.push_str(&wrap(&format!("off: {}", off.join(", ")), "  ", "    "));
    }
    out.push_str(&wrap(
        "Switch one with `acs features <name> on|off` (`acs features` explains each).",
        "  ",
        "  ",
    ));

    out.push_str("\nMain branch\n");
    let branch = &report.default_branch;
    let line = match &branch.found {
        None => format!(
            "not found (no remote HEAD, and the current branch is not main, master or develop), \
             so default_target_branch stays \"{}\". Set it in config.toml if that is wrong.",
            branch.target_branch
        ),
        Some(found) if !branch.written_to.is_empty() => {
            let mut places = Vec::new();
            if branch.written_to.contains(&"config.toml") {
                places.push("set as default_target_branch in config.toml");
            }
            if branch.written_to.contains(&"guards.toml") {
                places.push("added to protected_branches in guards.toml");
            }
            format!(
                "{} ({}): {}",
                found.name,
                found.source.describe(),
                places.join(", ")
            )
        }
        Some(found) if found.name == branch.target_branch && branch.protected => format!(
            "{} ({}), as the policy says",
            found.name,
            found.source.describe()
        ),
        Some(found) => format!(
            "{} ({}), which the policy does not match: see the warning below",
            found.name,
            found.source.describe()
        ),
    };
    out.push_str(&wrap(&line, "  ", "  "));

    out.push_str("\nFetch refspec for claims shared through git\n");
    if report.features_off.contains(&Feature::Sync) {
        out.push_str(&wrap(
            "not added: team sync is off, so nothing leaves this machine. `acs features sync \
             on` shares claims and session records through the git remote.",
            "  ",
            "  ",
        ));
    } else if report.remotes.is_empty() {
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
        HooksInstall::Done { hooks, note } => {
            for hook in hooks {
                let _ = writeln!(out, "  {:<10}{}", hook.action, hook.name);
            }
            if let Some(note) = note {
                out.push_str(&wrap(note, "  ", "  "));
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
