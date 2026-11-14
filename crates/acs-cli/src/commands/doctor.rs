//! `acs doctor`: checks the installation, one pass/warn/fail line each, and `--fix` repairs what
//! can be repaired safely.
//!
//! Checks: `ACS_HOME` and its permissions, repository id, the state database (opens, passes
//! `integrity_check`), the size of the copies kept for undo against their budget, the policy files,
//! whether `default_target_branch` names a branch that exists, the compiled-policy cache, git
//! hooks, `acs` on `PATH`,
//! the plugin of each detected harness, and whether the gate is off.
//!
//! Exit status is 1 when any check fails (after repairs), 0 otherwise; warnings do not fail.
//! `--fix` never turns the gate back on and never deletes the state database: a damaged one is
//! **moved aside** with a timestamp.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use acs_core::Timestamp;
use acs_git::RepoLayout;
use acs_paths::PathResolver;
use acs_policy::{LoadOptions, load};
use acs_store::{
    AcsHome, DbConfig, FLAG_GATE_OFF, HomeError, RepoHome, RepoIdentity, SnapshotStore, StateDb,
    StoreError,
};
use clap::Args;
use serde::Serialize;
use serde_json::json;

use super::init::branch;
use super::init::harness::{PluginState, detect, plugin_state};
use super::init::hooks::{self, HooksHealth, HooksInstall};
use super::init::policy::DEFAULT_TARGET_BRANCH;
use super::support::{CliError, OutputArgs, find_on_path, finish, print_json, wrap};
use crate::runtime::cleanup;

/// Arguments of `acs doctor`.
#[derive(Debug, Args)]
#[command(after_help = "Examples:\n  acs doctor\n  acs doctor --fix\n  acs doctor --json")]
pub struct DoctorArgs {
    /// Repair what can be repaired: tighten permissions, move a damaged state database aside
    /// and recreate it, clear a damaged policy cache, install missing git hooks.
    #[arg(long)]
    pub fix: bool,
    /// Output format.
    #[command(flatten)]
    pub output: OutputArgs,
}

/// How a check came out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Fine.
    Pass,
    /// Worth attention; ACS still works.
    Warn,
    /// Broken.
    Fail,
}

/// One check and its result; also an entry of the `--json` document.
#[derive(Debug, Clone, Serialize)]
pub struct Check {
    /// Stable identifier (`acs_home`, `state_db`, ...).
    pub id: String,
    /// Name shown to people.
    pub title: String,
    /// The result.
    pub status: Status,
    /// What was found, or what was repaired.
    pub message: String,
    /// True when `--fix` repaired it in this run.
    pub fixed: bool,
    /// What to do about it, when something is wrong.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

impl Check {
    fn new(id: &str, title: &str, status: Status, message: impl Into<String>) -> Self {
        Self {
            id: id.to_owned(),
            title: title.to_owned(),
            status,
            message: message.into(),
            fixed: false,
            hint: None,
        }
    }
    fn pass(id: &str, title: &str, message: impl Into<String>) -> Self {
        Self::new(id, title, Status::Pass, message)
    }
    fn warn(id: &str, title: &str, message: impl Into<String>) -> Self {
        Self::new(id, title, Status::Warn, message)
    }
    fn fail(id: &str, title: &str, message: impl Into<String>) -> Self {
        Self::new(id, title, Status::Fail, message)
    }
    fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
    fn fixed(mut self) -> Self {
        self.fixed = true;
        self
    }
}

/// Runs `acs doctor`.
#[must_use]
pub fn run(args: &DoctorArgs) -> ExitCode {
    finish(args.output.json, execute(args))
}

fn execute(args: &DoctorArgs) -> Result<ExitCode, CliError> {
    let cwd = std::env::current_dir()
        .map_err(|err| CliError::new("cwd", format!("cannot read the current directory: {err}")))?;
    let checks = run_checks(&cwd, args.fix);
    let count = |s: Status| checks.iter().filter(|c| c.status == s).count();
    let failed = count(Status::Fail);
    if args.output.json {
        print_json(&json!({
            "ok": failed == 0,
            "fix": args.fix,
            "checks": checks,
            "summary": {
                "pass": count(Status::Pass),
                "warn": count(Status::Warn),
                "fail": failed,
            },
        }));
    } else {
        print!("{}", render(&checks, args.fix));
    }
    Ok(if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

/// Runs every check against the repository around `cwd` (the repository-independent ones when
/// `cwd` is not in a git work tree).
#[must_use]
pub fn run_checks(cwd: &Path, fix: bool) -> Vec<Check> {
    let mut checks = Vec::new();
    let layout = acs_git::discover(cwd).ok();
    let home = AcsHome::from_env();
    // Looked at before anything below can create it.
    let home_existed = home.as_ref().is_ok_and(|h| h.root().exists());

    let (repo_home, id_check) = match (&home, &layout) {
        (Ok(home), Some(layout)) => resolve_repo(home, layout),
        _ => (None, None),
    };
    checks.push(check_home(&home, home_existed, repo_home.as_ref(), fix));
    let Some(layout) = layout else {
        checks.push(Check::warn(
            "repository",
            "Repository",
            format!(
                "{} is not inside a git work tree, so the repository checks were skipped",
                cwd.display()
            ),
        ));
        checks.push(check_acs_on_path(false));
        return checks;
    };
    checks.extend(id_check);
    let mut db_usable = false;
    if let Some(repo) = &repo_home {
        let (check, usable) = check_state_db(repo, fix);
        db_usable = usable;
        checks.push(check);
        checks.extend(check_snapshots(repo));
    }
    let policy_dir = layout.work_tree.join(".acs").join("policy");
    let policy_valid = {
        let (check, valid) = check_policy(&layout, &policy_dir);
        checks.push(check);
        valid
    };
    if policy_valid {
        checks.push(check_features(&layout, &policy_dir));
        checks.extend(check_target_branch(&layout, &policy_dir));
    }
    if let Some(repo) = &repo_home {
        checks.push(check_policy_cache(repo, &policy_dir, policy_valid, fix));
    }
    checks.push(check_hooks(&layout, fix));
    let detected = detect(&layout.work_tree);
    let plugin_states: Vec<PluginState> = detected
        .iter()
        .map(|found| plugin_state(found.harness, &layout.work_tree))
        .collect();
    checks.push(check_acs_on_path(
        plugin_states.contains(&PluginState::Installed),
    ));
    if detected.is_empty() {
        checks.push(Check::pass(
            "plugins",
            "Harness plugins",
            "no Claude Code or Antigravity detected on this machine or in this repository",
        ));
    }
    for (found, state) in detected.iter().zip(&plugin_states) {
        let id = format!("plugin_{}", found.id);
        let title = format!("{} plugin", found.name);
        checks.push(match state {
            PluginState::Installed => Check::pass(&id, &title, "the ACS plugin is installed"),
            PluginState::NotConfirmed => Check::warn(
                &id,
                &title,
                format!(
                    "{} is here but the ACS plugin was not found installed (the check reads \
                     settings files only and cannot see every scope)",
                    found.name
                ),
            )
            .hint(format!("install it with: {}", found.install.join(" && "))),
        });
    }
    checks.push(check_gate(repo_home.as_ref(), db_usable));
    checks
}

// ---------------------------------------------------------------------------------------------
// ACS_HOME and the repository id
// ---------------------------------------------------------------------------------------------

fn resolve_repo(home: &AcsHome, layout: &RepoLayout) -> (Option<RepoHome>, Option<Check>) {
    match RepoIdentity::resolve(home, &layout.work_tree) {
        Ok(identity) => {
            let id = identity.id().as_str().to_owned();
            let short: String = id.chars().take(12).collect();
            let check = Check::pass(
                "repo_id",
                "Repository id",
                format!("{short} ({:?} identity)", identity.kind()).to_lowercase(),
            );
            let check = match identity.migration_error() {
                Some(err) => Check::warn(
                    "repo_id",
                    "Repository id",
                    format!("{short}: moving state from the earlier id failed: {err}"),
                ),
                None => check,
            };
            (Some(identity.home(home)), Some(check))
        }
        Err(err) => (
            None,
            Some(Check::fail("repo_id", "Repository id", err.to_string())),
        ),
    }
}

fn check_home(
    home: &Result<AcsHome, HomeError>,
    existed: bool,
    repo: Option<&RepoHome>,
    fix: bool,
) -> Check {
    const ID: &str = "acs_home";
    const TITLE: &str = "ACS_HOME";
    let home = match home {
        Ok(home) => home,
        Err(err) => {
            return Check::fail(ID, TITLE, err.to_string())
                .hint("set ACS_HOME to an absolute path, or unset it to use ~/.acs");
        }
    };
    let root = home.root().display();
    if !existed {
        if fix {
            return match home.ensure() {
                Ok(()) => Check::pass(ID, TITLE, format!("{root} created (owner-only)")).fixed(),
                Err(err) => Check::fail(ID, TITLE, format!("cannot create {root}: {err}")),
            };
        }
        return Check::pass(
            ID,
            TITLE,
            format!("{root} does not exist yet; created on first use"),
        );
    }
    let windows_note = if cfg!(windows) {
        " Windows: access follows the ACL inherited from the parent directory (decision 0003); \
         keep ACS_HOME under your user profile."
    } else {
        ""
    };
    let loose = loose_permissions(home, repo);
    if loose.is_empty() {
        return Check::pass(ID, TITLE, format!("{root}, owner-only.{windows_note}"));
    }
    let list = loose
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    if fix {
        let failed: Vec<String> = loose
            .iter()
            .filter_map(|p| tighten(p).err().map(|e| format!("{}: {e}", p.display())))
            .collect();
        return if failed.is_empty() {
            Check::pass(
                ID,
                TITLE,
                format!("{root}: permissions tightened to owner-only"),
            )
            .fixed()
        } else {
            Check::fail(
                ID,
                TITLE,
                format!("cannot tighten permissions: {}", failed.join("; ")),
            )
        };
    }
    Check::fail(
        ID,
        TITLE,
        format!("readable by other users, which exposes claims and snapshots: {list}"),
    )
    .hint("run `acs doctor --fix`")
}

/// Paths under `ACS_HOME` that group or others can access (Unix; empty elsewhere).
fn loose_permissions(home: &AcsHome, repo: Option<&RepoHome>) -> Vec<PathBuf> {
    let mut paths = vec![
        home.root().to_path_buf(),
        home.repos_dir(),
        home.index_dir(),
    ];
    if let Some(repo) = repo {
        paths.extend([
            repo.dir().to_path_buf(),
            repo.snapshots_dir(),
            repo.cache_dir(),
            repo.pending_dir(),
        ]);
        let db = repo.state_db();
        for suffix in ["", "-wal", "-shm"] {
            let mut name = db.clone().into_os_string();
            name.push(suffix);
            paths.push(PathBuf::from(name));
        }
    }
    paths.into_iter().filter(|p| is_loose(p)).collect()
}

#[cfg(unix)]
fn is_loose(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o077 != 0)
}

#[cfg(not(unix))]
fn is_loose(_path: &Path) -> bool {
    false
}

#[cfg(unix)]
fn tighten(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let meta = fs::metadata(path)?;
    let mut mode = meta.permissions().mode() & !0o077;
    if meta.is_dir() {
        mode |= 0o700;
    }
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn tighten(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// State database
// ---------------------------------------------------------------------------------------------

/// Opens the database without creating it and runs the integrity check. `Ok(false)`: not
/// created yet.
fn verify_db(repo: &RepoHome) -> Result<bool, StoreError> {
    let config = DbConfig {
        create: false,
        ..DbConfig::default()
    };
    match StateDb::open_with(repo, &config) {
        Ok(db) => db.integrity_check().map(|()| true),
        Err(StoreError::Missing { .. }) => Ok(false),
        Err(err) => Err(err),
    }
}

fn check_state_db(repo: &RepoHome, fix: bool) -> (Check, bool) {
    const ID: &str = "state_db";
    const TITLE: &str = "State database";
    match verify_db(repo) {
        Ok(true) => (
            Check::pass(
                ID,
                TITLE,
                "opens, schema is current, integrity check passed",
            ),
            true,
        ),
        Ok(false) => (
            Check::pass(ID, TITLE, "not created yet; created on first use"),
            false,
        ),
        Err(err @ StoreError::Corrupt { .. }) => {
            if !fix {
                return (
                    Check::fail(ID, TITLE, err.to_string())
                        .hint("run `acs doctor --fix` to move it aside and start a new one"),
                    false,
                );
            }
            match quarantine_db(repo) {
                Ok(moved) => match StateDb::open(repo).and_then(|db| db.integrity_check()) {
                    Ok(()) => (
                        Check::pass(
                            ID,
                            TITLE,
                            format!(
                                "was corrupt: moved to {moved} and created a new one (claims and \
                                 seats were reset; snapshots are kept)"
                            ),
                        )
                        .fixed(),
                        true,
                    ),
                    Err(again) => (
                        Check::fail(
                            ID,
                            TITLE,
                            format!("moved the old file to {moved} but the new one fails: {again}"),
                        ),
                        false,
                    ),
                },
                Err(why) => (
                    Check::fail(ID, TITLE, format!("{err}; could not move it aside: {why}")),
                    false,
                ),
            }
        }
        Err(err) => (
            Check::fail(ID, TITLE, err.to_string()).hint(match err {
                StoreError::SchemaTooNew { .. } => "upgrade acs",
                StoreError::Locked => {
                    "close other acs or git processes using this repository and retry"
                }
                _ => "check the permissions of ACS_HOME and of state.db",
            }),
            false,
        ),
    }
}

/// Moves `state.db` (and its `-wal` and `-shm` companions) next to itself with a
/// `.corrupt-<unix time>` suffix; returns the new name of the main file.
fn quarantine_db(repo: &RepoHome) -> Result<String, String> {
    let stamp = Timestamp::now().as_second();
    let db = repo.state_db();
    let mut main_name = String::new();
    for suffix in ["", "-wal", "-shm"] {
        let mut from = db.clone().into_os_string();
        from.push(suffix);
        let from = PathBuf::from(from);
        if !from.exists() {
            continue;
        }
        let mut to = from.clone().into_os_string();
        to.push(format!(".corrupt-{stamp}"));
        let to = PathBuf::from(to);
        fs::rename(&from, &to).map_err(|err| format!("{}: {err}", from.display()))?;
        if suffix.is_empty() {
            main_name = to
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
        }
    }
    Ok(main_name)
}

// ---------------------------------------------------------------------------------------------
// Policy and its cache
// ---------------------------------------------------------------------------------------------

/// Which features the policy switches off. Informational: a feature that is off explains why
/// something a person expects (a blocked edit, a session in `acs log`) does not happen.
/// The copies kept for undo: their size against the budget. Doctor does not clean; `acs sync`
/// does, and it never removes the copies of a running session, which is the one reason a store
/// stays over budget.
fn check_snapshots(repo: &RepoHome) -> Option<Check> {
    const ID: &str = "snapshots";
    const TITLE: &str = "Copies kept for undo";
    // Nothing captured yet: do not create the store just to look at it.
    if !repo.snapshots_dir().join("index.db").exists() {
        return None;
    }
    let used = match SnapshotStore::open(repo).and_then(|store| store.stored_bytes()) {
        Ok(used) => used,
        Err(err) => {
            return Some(Check::warn(
                ID,
                TITLE,
                format!("cannot read the store: {err}"),
            ));
        }
    };
    let budget = cleanup::snapshot_budget();
    let message = format!(
        "{} used of {} per repository",
        human_bytes(used),
        human_bytes(budget)
    );
    Some(if used > budget {
        Check::warn(
            ID,
            TITLE,
            format!(
                "{message}: over budget. Copies of ended sessions are removed, oldest first, \
                 when `acs sync` runs; copies of a running session are never removed, so a \
                 long session can keep the store above the budget"
            ),
        )
        .hint("run `acs sync`; if it stays over budget, end the running sessions and run it again")
    } else {
        Check::pass(ID, TITLE, message)
    })
}

/// A size for people: `1.5 MiB`, `500 MiB`, `120 KiB`, `34 B`.
fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    #[allow(clippy::cast_precision_loss)]
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 || (value - value.round()).abs() < 0.05 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn check_features(layout: &RepoLayout, policy_dir: &Path) -> Check {
    const ID: &str = "features";
    const TITLE: &str = "Features";
    let case_insensitive = PathResolver::new(&layout.work_tree).is_ok_and(|r| r.case_insensitive());
    let features = load(policy_dir, LoadOptions { case_insensitive })
        .map(|loaded| loaded.policy.files().config.features)
        .unwrap_or_default();
    let off: Vec<&str> = features.off().into_iter().map(|f| f.key()).collect();
    if off.is_empty() {
        Check::pass(ID, TITLE, "all on")
    } else {
        Check::pass(
            ID,
            TITLE,
            format!("switched off: {} (see `acs features`)", off.join(", ")),
        )
    }
}

/// Whether `default_target_branch` names a branch that exists here or on a remote (decision
/// 0040). Claims only conflict between sessions with the same target branch, so a wrong name
/// changes which sessions collide. `None` when the branches cannot be listed.
fn check_target_branch(layout: &RepoLayout, policy_dir: &Path) -> Option<Check> {
    const ID: &str = "target_branch";
    const TITLE: &str = "Target branch";
    let case_insensitive = PathResolver::new(&layout.work_tree).is_ok_and(|r| r.case_insensitive());
    let target = load(policy_dir, LoadOptions { case_insensitive })
        .map(|loaded| loaded.policy.files().config.default_target_branch.clone())
        .unwrap_or_else(|_| DEFAULT_TARGET_BRANCH.to_owned());
    let known = branch::known_branches(&layout.work_tree)?;
    if known.contains(&target) {
        return Some(Check::pass(
            ID,
            TITLE,
            format!("default_target_branch = \"{target}\" exists"),
        ));
    }
    let mut message = format!(
        "default_target_branch = \"{target}\", but no branch of that name exists here or on a \
         remote. Claims only conflict between sessions with the same target branch."
    );
    if let Some(found) = branch::detect(&layout.work_tree) {
        let _ = write!(
            message,
            " The main branch looks like `{}` ({}).",
            found.name,
            found.source.describe()
        );
    }
    Some(
        Check::warn(ID, TITLE, message)
            .hint("set default_target_branch in .acs/policy/config.toml to the main branch"),
    )
}

fn check_policy(layout: &RepoLayout, policy_dir: &Path) -> (Check, bool) {
    const ID: &str = "policy";
    const TITLE: &str = "Policy";
    if !policy_dir.is_dir() {
        return (
            Check::warn(
                ID,
                TITLE,
                "no .acs/policy directory: only the built-in defaults apply",
            )
            .hint("run `acs init`"),
            true,
        );
    }
    let case_insensitive = PathResolver::new(&layout.work_tree).is_ok_and(|r| r.case_insensitive());
    match load(policy_dir, LoadOptions { case_insensitive }) {
        Ok(loaded) if loaded.warnings.is_empty() => {
            (Check::pass(ID, TITLE, "loads without warnings"), true)
        }
        Ok(loaded) => {
            let mut message = format!("loads with {} warning(s):", loaded.warnings.len());
            for warning in &loaded.warnings {
                let _ = write!(message, "\n- {warning}");
            }
            (Check::warn(ID, TITLE, message), true)
        }
        Err(err) => (
            Check::warn(
                ID,
                TITLE,
                format!(
                    "invalid, so the last valid policy (or only the built-in defaults) is \
                     enforced:\n{err}"
                ),
            )
            .hint("fix the file named above in .acs/policy/"),
            false,
        ),
    }
}

/// Whether `bytes` look like a policy cache entry (the shape `acs-policy` writes).
fn cache_entry_is_readable(bytes: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    value.get("format")?.as_u64()?;
    value.get("version")?.as_str()?;
    value.get("files")?.as_object()?;
    Some(value.get("hash")?.as_str()?.to_owned())
}

fn check_policy_cache(repo: &RepoHome, policy_dir: &Path, policy_valid: bool, fix: bool) -> Check {
    const ID: &str = "policy_cache";
    const TITLE: &str = "Policy cache";
    let path = repo.cache_dir().join(acs_policy::CACHE_FILE);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Check::pass(ID, TITLE, "not written yet; written on the next hook call");
        }
        Err(err) => {
            return Check::fail(ID, TITLE, format!("cannot read {}: {err}", path.display()))
                .hint("check the permissions of ACS_HOME");
        }
    };
    let Some(cached_hash) = cache_entry_is_readable(&bytes) else {
        if fix {
            return match fs::remove_file(&path) {
                Ok(()) => Check::pass(
                    ID,
                    TITLE,
                    "was unreadable: cleared; rebuilt from the policy files on the next call",
                )
                .fixed(),
                Err(err) => {
                    Check::fail(ID, TITLE, format!("unreadable, and cannot clear it: {err}"))
                }
            };
        }
        return Check::fail(
            ID,
            TITLE,
            format!(
                "{} is unreadable; policy is parsed on every call",
                path.display()
            ),
        )
        .hint("run `acs doctor --fix` to clear it");
    };
    let current = acs_policy::policy_hash(policy_dir)
        .ok()
        .map(|h| h.as_str().to_owned());
    let message = match (policy_valid, current) {
        (true, Some(hash)) if hash == cached_hash => "present and matches the policy files",
        (true, _) => "present but older than the policy files; refreshed on the next call",
        (false, _) => "present; holds the last valid policy while the files are invalid",
    };
    Check::pass(ID, TITLE, message)
}

// ---------------------------------------------------------------------------------------------
// Hooks, PATH, gate
// ---------------------------------------------------------------------------------------------

fn check_hooks(layout: &RepoLayout, fix: bool) -> Check {
    const ID: &str = "git_hooks";
    const TITLE: &str = "Git hooks";
    let mut health = hooks::health(layout);
    let mut repaired = false;
    if let HooksHealth::Unhealthy(_) = &health
        && fix
    {
        match hooks::install(layout) {
            HooksInstall::Done { .. } => {
                repaired = true;
                health = hooks::health(layout);
            }
            HooksInstall::Pending { note } => {
                return Check::warn(ID, TITLE, note);
            }
            HooksInstall::Failed { error } => {
                return Check::fail(ID, TITLE, format!("could not install them: {error}"));
            }
        }
    }
    match health {
        HooksHealth::Healthy => {
            let check = Check::pass(
                ID,
                TITLE,
                "pre-commit, prepare-commit-msg and pre-push are installed",
            );
            if repaired { check.fixed() } else { check }
        }
        HooksHealth::Unhealthy(why) => {
            Check::fail(ID, TITLE, why).hint("run `acs doctor --fix` or `acs init`")
        }
        HooksHealth::Unknown(why) => Check::warn(ID, TITLE, why),
    }
}

fn check_acs_on_path(plugin_installed: bool) -> Check {
    const ID: &str = "acs_on_path";
    const TITLE: &str = "acs on PATH";
    match find_on_path("acs") {
        Some(path) => Check::pass(ID, TITLE, path.display().to_string()),
        None => {
            let message = "`acs` does not resolve on PATH. Harness and git hooks run `acs`; \
                           without it nothing is enforced, and Antigravity denies every tool call.";
            let hint = "install acs into a directory on PATH";
            // With a plugin installed, its hooks run without the binary (Antigravity denies
            // everything, Claude Code runs tools unchecked): a failure. Otherwise a note.
            if plugin_installed {
                Check::fail(ID, TITLE, message).hint(hint)
            } else {
                Check::warn(ID, TITLE, message).hint(hint)
            }
        }
    }
}

fn check_gate(repo: Option<&RepoHome>, db_usable: bool) -> Check {
    const ID: &str = "gate";
    const TITLE: &str = "Gate";
    if !db_usable {
        return Check::pass(
            ID,
            TITLE,
            "on (no state database yet, so `acs off` is not set)",
        );
    }
    let Some(repo) = repo else {
        return Check::pass(ID, TITLE, "on");
    };
    let off = StateDb::open(repo).and_then(|db| db.flag_is_set(FLAG_GATE_OFF));
    match off {
        Ok(false) => Check::pass(ID, TITLE, "on: claims are enforced"),
        Ok(true) => Check::warn(
            ID,
            TITLE,
            "OFF (`acs off` is active): claims are not enforced; sealed paths still hold",
        )
        .hint("run `acs on` to enforce claims again"),
        Err(err) => Check::fail(ID, TITLE, format!("cannot read the gate flag: {err}")),
    }
}

// ---------------------------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------------------------

/// The human view, every line at most 80 columns.
#[must_use]
pub fn render(checks: &[Check], fix: bool) -> String {
    let mut out = String::from("acs doctor\n\n");
    for check in checks {
        let tag = match (check.status, check.fixed) {
            (_, true) => "fixed",
            (Status::Pass, false) => "ok",
            (Status::Warn, false) => "WARN",
            (Status::Fail, false) => "FAIL",
        };
        let _ = writeln!(out, "{tag:<6}{}", check.title);
        out.push_str(&wrap(&check.message, "      ", "      "));
        if let Some(hint) = &check.hint {
            out.push_str(&wrap(&format!("-> {hint}"), "      ", "         "));
        }
    }
    let count = |s: Status| checks.iter().filter(|c| c.status == s).count();
    let repaired = checks.iter().filter(|c| c.fixed).count();
    let _ = write!(
        out,
        "\n{} ok, {} warning(s), {} failure(s)",
        count(Status::Pass),
        count(Status::Warn),
        count(Status::Fail)
    );
    if repaired > 0 {
        let _ = write!(out, ", {repaired} repaired");
    }
    out.push('\n');
    let fixable = |c: &Check| {
        c.status == Status::Fail && c.hint.as_deref().is_some_and(|h| h.contains("--fix"))
    };
    if !fix && checks.iter().any(fixable) {
        out.push_str("Run `acs doctor --fix` to repair what can be repaired.\n");
    }
    out
}
