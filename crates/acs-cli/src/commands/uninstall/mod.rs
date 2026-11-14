//! `acs uninstall`: takes ACS out of a repository (issue 63, decision 0039).
//!
//! Always, unless `--dry-run`:
//!
//! 1. the ACS block in each git hook, restoring a hook ACS wrapped (`acs_git::hooks::uninstall`);
//! 2. the `refs/acs/*` fetch refspec of every remote, and the local `refs/acs/*` refs and their
//!    remote-tracking copies;
//! 3. the ACS plugin entries in the project's Claude Code settings (`.claude/settings.json` and
//!    `.claude/settings.local.json`), and the Antigravity plugin folder `.agents/plugins/acs/`.
//!
//! On request, each on its own (a flag, or a yes at its question in a terminal):
//!
//! - `--policy`: the policy folder `.acs/policy/` (the team's; the deletion must be committed);
//! - `--state`: the repository's local state in ACS's home folder;
//! - `--remote-refs`: the `refs/acs/*` refs on every remote.
//!
//! After `acs init` and `acs uninstall`, the repository, its git config and its hooks are as they
//! were before `init`. The plan is shown before anything changes; `--dry-run` shows it only.
//! Agents may not run it (it is not in the read-only list of `acs_core::engine::protect`).

pub mod settings;

use std::fmt::Write as _;
use std::io::{BufRead as _, IsTerminal as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use acs_git::RepoLayout;
use acs_git::hooks::{self, HookRemoval, HooksError, RemoveAction};
use acs_store::{AcsHome, RepoIdentity};
use clap::Args;
use serde::Serialize;

use self::settings::Edit;
use super::init::harness::{
    CLAUDE_CODE_PLUGIN, claude_code_project_settings, claude_code_user_settings,
    settings_enable_acs,
};
use super::plugin::ANTIGRAVITY_PLUGIN_DIR;
use super::support::{CliError, OutputArgs, ellipsize_start, finish, print_json, wrap};

/// Arguments of `acs uninstall`.
#[derive(Debug, Args)]
#[command(
    after_help = "Without a flag for them, the policy folder, the local state and the refs \
                  on the remotes are kept; in a terminal, `acs uninstall` asks about each. \
                  `acs init` sets ACS up again.\n\nExamples:\n  acs uninstall --dry-run\n  acs \
                  uninstall\n  acs uninstall --all --json"
)]
pub struct UninstallArgs {
    /// Show what would be removed and change nothing.
    #[arg(long)]
    pub dry_run: bool,
    /// Also delete .acs/policy/, the team's policy (commit the deletion to share it).
    #[arg(long)]
    pub policy: bool,
    /// Also delete this repository's state in ACS's home folder: claims, session history and
    /// the copies kept for undo, shared by every clone of it on this machine.
    #[arg(long)]
    pub state: bool,
    /// Also delete the refs/acs/* refs on every remote: the claims and session records shared
    /// with the team.
    #[arg(long)]
    pub remote_refs: bool,
    /// All three of --policy, --state and --remote-refs.
    #[arg(long)]
    pub all: bool,
    /// Output format.
    #[command(flatten)]
    pub output: OutputArgs,
}

/// What became of one step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StepState {
    /// It was removed.
    Done,
    /// Dry run: it would be removed.
    Planned,
    /// Optional and not chosen: it was kept.
    Kept,
    /// There was nothing to remove.
    Nothing,
    /// Removing it failed; see `error`.
    Failed,
}

/// One step of the uninstall; also an entry of the `--json` document.
#[derive(Debug, Clone, Serialize)]
pub struct Step {
    /// Stable identifier: `git_hooks`, `fetch_refspecs`, `local_refs`, `claude_code_settings`,
    /// `antigravity_plugin`, `policy`, `state`, `remote_refs`.
    pub id: &'static str,
    /// For people.
    pub title: &'static str,
    /// Only done when asked for (a flag, or a yes in a terminal).
    pub optional: bool,
    /// What became of it.
    pub state: StepState,
    /// What it removes (or would), one entry each.
    pub items: Vec<String>,
    /// Why it failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The `--json` document.
#[derive(Debug, Clone, Serialize)]
pub struct UninstallReport {
    /// False when a step failed.
    pub ok: bool,
    /// Whether this was a dry run.
    pub dry_run: bool,
    /// Whether anything changed.
    pub changed: bool,
    /// The work tree.
    pub worktree: String,
    /// The steps, in the order they run.
    pub steps: Vec<Step>,
    /// What the person should do next.
    pub notes: Vec<String>,
}

/// Everything found to remove, before anything changes.
struct Plan {
    layout: RepoLayout,
    hooks: Result<Vec<HookRemoval>, HooksError>,
    /// `(remote, refspec)` pairs present in the git config.
    refspecs: Vec<(String, String)>,
    local_refs: Vec<String>,
    /// Settings files with ACS entries, and the edit that removes them.
    settings: Vec<(PathBuf, Edit)>,
    settings_errors: Vec<String>,
    antigravity: Option<PathBuf>,
    policy: Option<PathBuf>,
    state: Result<Vec<PathBuf>, String>,
    remotes: Vec<String>,
}

/// Runs `acs uninstall`.
#[must_use]
pub fn run(args: &UninstallArgs) -> ExitCode {
    finish(args.output.json, execute(args))
}

fn execute(args: &UninstallArgs) -> Result<ExitCode, CliError> {
    let cwd = std::env::current_dir()
        .map_err(|err| CliError::new("cwd", format!("cannot read the current directory: {err}")))?;
    let layout =
        acs_git::discover(&cwd).map_err(|err| CliError::new("not_a_repo", err.to_string()))?;
    let plan = plan(layout);
    let interactive = !args.output.json
        && !args.dry_run
        && std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal();
    let mut choice = Choice {
        policy: args.policy || args.all,
        state: args.state || args.all,
        remote_refs: args.remote_refs || args.all,
    };
    if interactive {
        print!("{}", render_plan(&plan, choice));
        choice = ask_optional(&plan, choice);
    }
    let report = carry_out(&plan, choice, args.dry_run);
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

/// Which optional steps to take.
#[derive(Debug, Clone, Copy)]
struct Choice {
    policy: bool,
    state: bool,
    remote_refs: bool,
}

// ---------------------------------------------------------------------------------------------
// Git
// ---------------------------------------------------------------------------------------------

fn git(work_tree: &Path) -> Command {
    let mut command = Command::new("git");
    command.arg("-C").arg(work_tree);
    for var in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_PREFIX",
    ] {
        command.env_remove(var);
    }
    // Never wait for a password: a remote that needs one is reported as failed.
    command.env("GIT_TERMINAL_PROMPT", "0").stdin(Stdio::null());
    command
}

/// Runs git and returns its stdout, or its stderr as the error.
fn git_output(work_tree: &Path, args: &[&str]) -> Result<String, String> {
    let out = git(work_tree)
        .args(args)
        .output()
        .map_err(|err| format!("cannot run git: {err}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(format!(
            "git {} failed: {}",
            args.first().copied().unwrap_or_default(),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// `text` as a POSIX extended regular expression that matches only itself.
fn regex_literal(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 2);
    for c in text.chars() {
        if !c.is_ascii_alphanumeric() && c != '/' && c != '-' && c != '_' {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn fetch_refspecs(work_tree: &Path, remotes: &[String]) -> Vec<(String, String)> {
    let mut found = Vec::new();
    for remote in remotes {
        let spec = acs_git::refs::fetch_refspec(remote);
        let key = format!("remote.{remote}.fetch");
        let values = git_output(work_tree, &["config", "--get-all", &key]).unwrap_or_default();
        if values.lines().any(|line| line.trim() == spec) {
            found.push((remote.clone(), spec));
        }
    }
    found
}

fn local_acs_refs(work_tree: &Path, remotes: &[String]) -> Vec<String> {
    let mut patterns = vec!["refs/acs".to_owned()];
    patterns.extend(remotes.iter().map(|r| format!("refs/remotes/{r}/acs")));
    let mut args = vec!["for-each-ref", "--format=%(refname)"];
    args.extend(patterns.iter().map(String::as_str));
    git_output(work_tree, &args)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn remote_acs_refs(work_tree: &Path, remote: &str) -> Result<Vec<String>, String> {
    let listing = git_output(work_tree, &["ls-remote", remote, "refs/acs/*"])?;
    Ok(listing
        .lines()
        .filter_map(|line| line.split('\t').nth(1))
        .map(str::to_owned)
        .collect())
}

// ---------------------------------------------------------------------------------------------
// Planning
// ---------------------------------------------------------------------------------------------

fn plan(layout: RepoLayout) -> Plan {
    let work_tree = layout.work_tree.clone();
    let remotes = acs_git::repo::remotes(&work_tree).unwrap_or_default();
    let mut settings = Vec::new();
    let mut settings_errors = Vec::new();
    for file in claude_code_project_settings(&work_tree) {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let (enabled, marketplace) = acs_entries();
        match settings::remove_entries(&text, &[enabled, marketplace]) {
            Ok(Edit::Unchanged) => {}
            Ok(edit) => settings.push((file, edit)),
            Err(err) => settings_errors.push(format!(
                "{}: not valid JSON, left as it is ({err})",
                relative(&work_tree, &file)
            )),
        }
    }
    let antigravity = Some(work_tree.join(ANTIGRAVITY_PLUGIN_DIR)).filter(|d| d.is_dir());
    let policy = Some(work_tree.join(".acs").join("policy")).filter(|d| d.is_dir());
    let state = AcsHome::from_env()
        .map_err(|err| err.to_string())
        .and_then(|home| local_state(&home, &work_tree));
    Plan {
        hooks: hooks::uninstall_plan(&layout),
        refspecs: fetch_refspecs(&work_tree, &remotes),
        local_refs: local_acs_refs(&work_tree, &remotes),
        settings,
        settings_errors,
        antigravity,
        policy,
        state,
        remotes,
        layout,
    }
}

/// This repository's files in ACS's home folder. Resolving the id may write its cache entry;
/// when that entry did not exist before, it is taken away again and not reported, so looking
/// leaves no trace.
fn local_state(home: &AcsHome, work_tree: &Path) -> Result<Vec<PathBuf>, String> {
    let cache = RepoIdentity::cache_file_for(home, work_tree).ok();
    let cached_before = cache.as_ref().is_some_and(|c| c.exists());
    let identity = RepoIdentity::resolve(home, work_tree).map_err(|err| err.to_string())?;
    let mut files = identity.local_files(home);
    if !cached_before && let Some(cache) = &cache {
        let _ = std::fs::remove_file(cache);
        files.retain(|f| f != cache);
    }
    Ok(files)
}

/// The two settings entries the ACS plugin adds: `enabledPlugins."acs@acs-plugins"` and
/// `extraKnownMarketplaces."acs-plugins"`.
fn acs_entries() -> ((&'static str, &'static str), (&'static str, &'static str)) {
    let marketplace = CLAUDE_CODE_PLUGIN
        .split_once('@')
        .map_or(CLAUDE_CODE_PLUGIN, |(_, m)| m);
    (
        ("enabledPlugins", CLAUDE_CODE_PLUGIN),
        ("extraKnownMarketplaces", marketplace),
    )
}

fn relative(work_tree: &Path, path: &Path) -> String {
    path.strip_prefix(work_tree)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn hook_item(work_tree: &Path, removal: &HookRemoval) -> Option<String> {
    let path = relative(work_tree, &removal.path);
    match removal.action {
        RemoveAction::Removed => Some(format!("{path}: the ACS block (the rest of it stays)")),
        RemoveAction::Deleted => Some(format!("{path}: the file, which holds only ACS's block")),
        RemoveAction::Restored => Some(format!(
            "{path}: ACS's wrapper ({}.acs-orig moves back)",
            removal.name
        )),
        RemoveAction::NotInstalled => None,
    }
}

fn settings_item(work_tree: &Path, file: &Path, edit: &Edit) -> String {
    let path = relative(work_tree, file);
    match edit {
        Edit::Unchanged => path,
        Edit::Changed { removed, .. } => format!("{path}: {}", removed.join(", ")),
        Edit::Empty { removed } => format!(
            "{path}: {} (the file is deleted: nothing else is in it)",
            removed.join(", ")
        ),
    }
}

fn policy_item(work_tree: &Path, dir: &Path) -> String {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|e| {
                    let name = e.file_name().to_string_lossy().into_owned();
                    if e.path().is_dir() { name + "/" } else { name }
                })
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    format!("{}/ ({})", relative(work_tree, dir), names.join(", "))
}

/// The steps as the plan shows them, before anything is done.
fn planned_steps(plan: &Plan, choice: Choice) -> Vec<Step> {
    let work_tree = &plan.layout.work_tree;
    let step = |id, title, optional, items: Vec<String>| Step {
        id,
        title,
        optional,
        state: if items.is_empty() {
            StepState::Nothing
        } else {
            StepState::Planned
        },
        items,
        error: None,
    };
    let mut steps = Vec::new();
    let mut hooks_step = match &plan.hooks {
        Ok(removals) => step(
            "git_hooks",
            "Git hooks",
            false,
            removals
                .iter()
                .filter_map(|r| hook_item(work_tree, r))
                .collect(),
        ),
        Err(_) => step("git_hooks", "Git hooks", false, Vec::new()),
    };
    if let Err(err) = &plan.hooks
        && !matches!(err, HooksError::Disabled(_))
    {
        hooks_step.state = StepState::Failed;
        hooks_step.error = Some(err.to_string());
    }
    steps.push(hooks_step);
    steps.push(step(
        "fetch_refspecs",
        "Fetch refspecs",
        false,
        plan.refspecs
            .iter()
            .map(|(remote, spec)| format!("remote.{remote}.fetch = {spec}"))
            .collect(),
    ));
    steps.push(step(
        "local_refs",
        "Local ACS refs",
        false,
        plan.local_refs.clone(),
    ));
    let mut settings_step = step(
        "claude_code_settings",
        "Claude Code plugin",
        false,
        plan.settings
            .iter()
            .map(|(file, edit)| settings_item(work_tree, file, edit))
            .collect(),
    );
    if !plan.settings_errors.is_empty() {
        settings_step.state = StepState::Failed;
        settings_step.error = Some(plan.settings_errors.join("; "));
    }
    steps.push(settings_step);
    steps.push(step(
        "antigravity_plugin",
        "Antigravity plugin",
        false,
        plan.antigravity
            .iter()
            .map(|dir| format!("{}/", relative(work_tree, dir)))
            .collect(),
    ));
    let mut policy = step(
        "policy",
        "Policy",
        true,
        plan.policy
            .iter()
            .map(|dir| policy_item(work_tree, dir))
            .collect(),
    );
    if policy.state == StepState::Planned && !choice.policy {
        policy.state = StepState::Kept;
    }
    steps.push(policy);
    let mut state = match &plan.state {
        Ok(paths) => step(
            "state",
            "Local state",
            true,
            paths.iter().map(|p| p.display().to_string()).collect(),
        ),
        Err(err) => {
            let mut s = step("state", "Local state", true, Vec::new());
            s.state = StepState::Failed;
            s.error = Some(format!("cannot find it: {err}"));
            s
        }
    };
    if state.state == StepState::Planned && !choice.state {
        state.state = StepState::Kept;
    }
    steps.push(state);
    let mut remote = step(
        "remote_refs",
        "Refs on the remotes",
        true,
        plan.remotes
            .iter()
            .map(|r| format!("refs/acs/* on {r}"))
            .collect(),
    );
    if remote.state == StepState::Planned && !choice.remote_refs {
        remote.state = StepState::Kept;
    }
    steps.push(remote);
    steps
}

// ---------------------------------------------------------------------------------------------
// Asking
// ---------------------------------------------------------------------------------------------

fn ask(question: &str) -> bool {
    print!("{question} [y/N] ");
    let _ = std::io::stdout().flush();
    let mut answer = String::new();
    if std::io::stdin().lock().read_line(&mut answer).is_err() {
        return false;
    }
    matches!(answer.trim(), "y" | "Y" | "yes" | "Yes" | "YES")
}

/// Asks about each optional step that has something to remove and no flag for it.
fn ask_optional(plan: &Plan, mut choice: Choice) -> Choice {
    let mut asked = false;
    if !choice.policy && plan.policy.is_some() {
        asked = true;
        choice.policy = ask(
            "Delete .acs/policy/? It is the team's policy; commit the deletion to remove ACS \
             for everyone.",
        );
    }
    if !choice.state && plan.state.as_ref().is_ok_and(|p| !p.is_empty()) {
        asked = true;
        choice.state = ask(
            "Delete this repository's local ACS state (claims, session history, copies kept \
             for undo)? Every clone of it on this machine shares it.",
        );
    }
    if !choice.remote_refs && !plan.remotes.is_empty() {
        asked = true;
        choice.remote_refs = ask(&format!(
            "Delete the refs/acs/* refs on {} (claims and session records shared with the \
             team)?",
            plan.remotes.join(", ")
        ));
    }
    if asked {
        println!();
    }
    choice
}

// ---------------------------------------------------------------------------------------------
// Doing
// ---------------------------------------------------------------------------------------------

/// Removes a directory if it is empty; anything else is left alone.
fn remove_if_empty(dir: &Path) {
    if std::fs::read_dir(dir).is_ok_and(|mut entries| entries.next().is_none()) {
        let _ = std::fs::remove_dir(dir);
    }
}

/// Removes `dir` and its subfolders as long as they hold no files.
fn remove_empty_tree(dir: &Path) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                remove_empty_tree(&entry.path());
            }
        }
    }
    remove_if_empty(dir);
}

fn remove_path(path: &Path) -> std::io::Result<()> {
    if std::fs::symlink_metadata(path)?.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

fn fail(step: &mut Step, error: String) {
    step.state = StepState::Failed;
    step.error = Some(match step.error.take() {
        Some(earlier) => format!("{earlier}; {error}"),
        None => error,
    });
}

/// Carries out the plan (or, with `dry_run`, only reports it).
fn carry_out(plan: &Plan, choice: Choice, dry_run: bool) -> UninstallReport {
    let work_tree = &plan.layout.work_tree;
    let mut steps = planned_steps(plan, choice);

    // Remote refs are listed only when they are to go: listing needs the network.
    if let Some(step) = steps.iter_mut().find(|s| s.id == "remote_refs")
        && step.state == StepState::Planned
    {
        step.items.clear();
        let mut errors = Vec::new();
        let mut found: Vec<(String, Vec<String>)> = Vec::new();
        for remote in &plan.remotes {
            match remote_acs_refs(work_tree, remote) {
                Ok(refs) if refs.is_empty() => {}
                Ok(refs) => {
                    step.items
                        .extend(refs.iter().map(|r| format!("{remote}: {r}")));
                    found.push((remote.clone(), refs));
                }
                Err(err) => errors.push(format!("{remote}: {err}")),
            }
        }
        if step.items.is_empty() {
            step.state = StepState::Nothing;
        }
        if !dry_run {
            for (remote, refs) in &found {
                // `--no-verify`: deleting ACS's own refs is not the push a pre-push hook is for.
                let mut args = vec![
                    "push",
                    "--no-verify",
                    "--quiet",
                    remote.as_str(),
                    "--delete",
                ];
                args.extend(refs.iter().map(String::as_str));
                if let Err(err) = git_output(work_tree, &args) {
                    errors.push(format!("{remote}: {err}"));
                }
            }
        }
        if !errors.is_empty() {
            fail(step, errors.join("; "));
        }
    }

    if !dry_run {
        for step in &mut steps {
            if step.state != StepState::Planned {
                continue;
            }
            let result = match step.id {
                "git_hooks" => hooks::uninstall(&plan.layout)
                    .map(|_| ())
                    .map_err(|err| err.to_string()),
                "fetch_refspecs" => remove_refspecs(work_tree, &plan.refspecs),
                "local_refs" => {
                    let result = delete_local_refs(work_tree, &plan.local_refs);
                    // git keeps the emptied folders of loose refs; take them away too.
                    let refs = plan.layout.common_dir.join("refs");
                    remove_empty_tree(&refs.join("acs"));
                    for remote in &plan.remotes {
                        remove_empty_tree(&refs.join("remotes").join(remote).join("acs"));
                    }
                    result
                }
                "claude_code_settings" => apply_settings(work_tree, &plan.settings),
                "antigravity_plugin" => plan.antigravity.as_ref().map_or(Ok(()), |dir| {
                    let result = std::fs::remove_dir_all(dir).map_err(|err| err.to_string());
                    if let Some(plugins) = dir.parent() {
                        remove_if_empty(plugins);
                        plugins.parent().into_iter().for_each(remove_if_empty);
                    }
                    result
                }),
                "policy" => plan.policy.as_ref().map_or(Ok(()), |dir| {
                    let result = std::fs::remove_dir_all(dir).map_err(|err| err.to_string());
                    dir.parent().into_iter().for_each(remove_if_empty);
                    result
                }),
                "state" => match &plan.state {
                    Ok(paths) => delete_state(paths),
                    Err(err) => Err(err.clone()),
                },
                // Done above.
                _ => Ok(()),
            };
            match result {
                Ok(()) => step.state = StepState::Done,
                Err(err) => fail(step, err),
            }
        }
    }

    let changed = steps.iter().any(|s| s.state == StepState::Done);
    let notes = notes(plan, &steps, dry_run);
    UninstallReport {
        ok: steps.iter().all(|s| s.state != StepState::Failed),
        dry_run,
        changed,
        worktree: work_tree.to_string_lossy().into_owned(),
        steps,
        notes,
    }
}

fn remove_refspecs(work_tree: &Path, refspecs: &[(String, String)]) -> Result<(), String> {
    for (remote, spec) in refspecs {
        let key = format!("remote.{remote}.fetch");
        let pattern = format!("^{}$", regex_literal(spec));
        git_output(work_tree, &["config", "--unset-all", &key, &pattern])?;
    }
    Ok(())
}

fn delete_local_refs(work_tree: &Path, refs: &[String]) -> Result<(), String> {
    let mut child = git(work_tree)
        .args(["update-ref", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("cannot run git: {err}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let script: String = refs.iter().map(|r| format!("delete {r}\n")).collect();
        stdin
            .write_all(script.as_bytes())
            .map_err(|err| format!("cannot write to git update-ref: {err}"))?;
    }
    let out = child
        .wait_with_output()
        .map_err(|err| format!("git update-ref failed: {err}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "git update-ref failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

fn apply_settings(work_tree: &Path, edits: &[(PathBuf, Edit)]) -> Result<(), String> {
    for (file, edit) in edits {
        let result = match edit {
            Edit::Unchanged => Ok(()),
            Edit::Changed { text, .. } => std::fs::write(file, text),
            Edit::Empty { .. } => std::fs::remove_file(file),
        };
        result.map_err(|err| format!("{}: {err}", relative(work_tree, file)))?;
        if matches!(edit, Edit::Empty { .. }) {
            file.parent().into_iter().for_each(remove_if_empty);
        }
    }
    Ok(())
}

fn delete_state(paths: &[PathBuf]) -> Result<(), String> {
    let mut errors = Vec::new();
    for path in paths {
        if let Err(err) = remove_path(path) {
            errors.push(format!("{}: {err}", path.display()));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{}. A running Claude Code session, `acs watch` or the VS Code extension may hold \
             it open: close them and run `acs uninstall --state` again.",
            errors.join("; ")
        ))
    }
}

fn notes(plan: &Plan, steps: &[Step], dry_run: bool) -> Vec<String> {
    let state_of = |id: &str| {
        steps
            .iter()
            .find(|s| s.id == id)
            .map_or(StepState::Nothing, |s| s.state)
    };
    let mut notes = Vec::new();
    if dry_run {
        notes.push(
            "Dry run: nothing was changed. Run `acs uninstall` without --dry-run to do it."
                .to_owned(),
        );
        return notes;
    }
    let mut to_commit = Vec::new();
    if state_of("policy") == StepState::Done {
        to_commit.push(".acs/policy/");
    }
    if state_of("claude_code_settings") == StepState::Done
        && plan
            .settings
            .iter()
            .any(|(file, _)| file.ends_with("settings.json"))
    {
        to_commit.push(".claude/settings.json");
    }
    if state_of("antigravity_plugin") == StepState::Done {
        to_commit.push(ANTIGRAVITY_PLUGIN_DIR);
    }
    if plan.hooks.as_ref().is_ok_and(|h| {
        h.iter()
            .any(|r| relative(&plan.layout.work_tree, &r.path).starts_with(".husky/"))
    }) && state_of("git_hooks") == StepState::Done
    {
        to_commit.push(".husky/");
    }
    if !to_commit.is_empty() {
        notes.push(format!(
            "Commit the change to {} so teammates' clones stop using ACS.",
            to_commit.join(", ")
        ));
    }
    if state_of("policy") == StepState::Kept {
        notes.push(
            "The policy in .acs/policy/ is kept: `acs init` sets ACS up again with it, and \
             `acs uninstall --policy` deletes it."
                .to_owned(),
        );
    }
    if claude_code_user_settings().is_some_and(|file| settings_enable_acs(&file)) {
        notes.push(format!(
            "The ACS plugin is still enabled for your user in Claude Code, which gates every \
             repository you open. Remove it with `claude plugin uninstall {CLAUDE_CODE_PLUGIN} \
             --scope user`."
        ));
    }
    notes.push(
        "The `acs` program itself stays installed; remove it the way you installed it.".to_owned(),
    );
    notes
}

// ---------------------------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------------------------

/// Items shown per step before the rest is counted.
const SHOWN_ITEMS: usize = 8;

fn state_word(step: &Step) -> String {
    match step.state {
        StepState::Done => "removed".to_owned(),
        StepState::Planned => "to remove".to_owned(),
        StepState::Nothing => "nothing to remove".to_owned(),
        StepState::Failed => "FAILED".to_owned(),
        StepState::Kept => {
            let flag = match step.id {
                "policy" => "--policy",
                "state" => "--state",
                _ => "--remote-refs",
            };
            format!("kept ({flag} removes it)")
        }
    }
}

fn render_steps(out: &mut String, steps: &[Step]) {
    for step in steps {
        out.push_str(&wrap(
            &format!("{}: {}", step.title, state_word(step)),
            "",
            "  ",
        ));
        if step.state == StepState::Nothing {
            continue;
        }
        for item in step.items.iter().take(SHOWN_ITEMS) {
            // A long path cannot wrap: keep its end, which tells it apart.
            if item.len() > 78 && !item.contains(' ') {
                let _ = writeln!(out, "  {}", ellipsize_start(item, 78));
            } else {
                out.push_str(&wrap(item, "  ", "    "));
            }
        }
        if step.items.len() > SHOWN_ITEMS {
            let _ = writeln!(out, "  ... and {} more", step.items.len() - SHOWN_ITEMS);
        }
        if let Some(error) = &step.error {
            out.push_str(&wrap(error, "  ", "  "));
        }
    }
}

/// The plan as shown in a terminal before the questions.
fn render_plan(plan: &Plan, choice: Choice) -> String {
    let mut out = format!(
        "acs uninstall: {}\n\nThis removes:\n",
        ellipsize_start(
            &plan.layout.work_tree.to_string_lossy(),
            80 - "acs uninstall: ".len()
        )
    );
    let mut steps = planned_steps(plan, choice);
    for step in &mut steps {
        if step.state == StepState::Kept {
            step.state = StepState::Planned;
        }
    }
    let (always, optional): (Vec<Step>, Vec<Step>) = steps.into_iter().partition(|s| !s.optional);
    render_steps(&mut out, &always);
    out.push_str("\nAnd, if you say yes below:\n");
    render_steps(&mut out, &optional);
    out.push('\n');
    out
}

/// The human view of the result, at most 80 columns wide.
#[must_use]
pub fn render(report: &UninstallReport) -> String {
    let title = if report.dry_run {
        "acs uninstall --dry-run: "
    } else {
        "acs uninstall: "
    };
    let mut out = format!(
        "{title}{}\n\n",
        ellipsize_start(&report.worktree, 80 - title.len())
    );
    render_steps(&mut out, &report.steps);
    for note in &report.notes {
        out.push('\n');
        out.push_str(&wrap(note, "", ""));
    }
    if !report.dry_run && report.ok && !report.changed {
        out.push_str("\nNothing to remove: ACS is not set up in this repository.\n");
    }
    out
}
