//! `acs plugin install <harness>`: installs the ACS plugin into an agent harness.
//!
//! Keyed by harness name, so a harness joins by getting one arm in [`install`]. Claude Code is the
//! only one for now; Antigravity is added when it returns (decision 0019).
//!
//! `acs` must resolve on `PATH` first: the plugin's hooks run `acs`, and without it they either
//! check nothing (Claude Code) or deny every tool call (Antigravity).
//!
//! The default scope is **project**, not the harness's own default of user: the hook gates every
//! git repository it runs in, using the built-in guards where there is no `.acs/policy`, so a
//! user-scope plugin changes how agents behave in repositories that never adopted ACS (decision
//! 0021). Running it again changes nothing.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use clap::{Args, Subcommand, ValueEnum};
use serde::Serialize;

use super::init::harness::{CLAUDE_CODE_PLUGIN, HARNESSES, Harness};
use super::support::{
    CliError, OutputArgs, find_on_path, find_spawnable_on_path, finish, print_json, wrap,
};

/// The marketplace the plugin comes from (this repository, `.claude-plugin/marketplace.json`).
pub const DEFAULT_MARKETPLACE: &str = "acs-hq/acs";

/// Arguments of `acs plugin`.
#[derive(Debug, Args)]
pub struct PluginArgs {
    /// What to do.
    #[command(subcommand)]
    pub command: PluginCommand,
}

/// The subcommands of `acs plugin`.
#[derive(Debug, Subcommand)]
pub enum PluginCommand {
    /// Install the ACS plugin into a harness and enable it.
    Install(InstallArgs),
}

/// Arguments of `acs plugin install`.
#[derive(Debug, Args)]
#[command(
    after_help = "Examples:\n  acs plugin install claude-code\n  acs plugin install claude-code \
                  --scope user\n  acs plugin install claude-code --marketplace ./my-checkout\n\n\
                  Safe to run again: a plugin that is already enabled is left alone."
)]
pub struct InstallArgs {
    /// The harness, by adapter name (`claude-code`).
    #[arg(value_name = "HARNESS")]
    pub harness: String,
    /// Where to enable it. `project` is shared through `.claude/settings.json`; `user` gates every
    /// git repository you open with the harness, even those that never adopted ACS.
    #[arg(long, value_enum, default_value_t = Scope::Project)]
    pub scope: Scope,
    /// Where the plugin comes from: `owner/repo`, a git URL or a local checkout.
    #[arg(long, value_name = "SOURCE", default_value = DEFAULT_MARKETPLACE)]
    pub marketplace: String,
    /// Output format.
    #[command(flatten)]
    pub output: OutputArgs,
}

/// Where a plugin is enabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// This repository, for everyone: committed in `.claude/settings.json`.
    Project,
    /// This repository, for you: `.claude/settings.local.json`.
    Local,
    /// Every repository you open.
    User,
}

impl Scope {
    /// The name the harness's command line uses.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Local => "local",
            Self::User => "user",
        }
    }
}

/// One command run against the harness.
#[derive(Debug, Clone, Serialize)]
pub struct Step {
    /// The command line, for people.
    pub command: String,
    /// Whether it exited with success.
    pub ok: bool,
    /// What it said, when it failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}

/// What `acs plugin install` did; also the `--json` document.
#[derive(Debug, Clone, Serialize)]
pub struct InstallReport {
    /// False when a step failed or the plugin did not end up enabled.
    pub ok: bool,
    /// Whether anything changed.
    pub changed: bool,
    /// The harness.
    pub harness: &'static str,
    /// The scope asked for.
    pub scope: Scope,
    /// The plugin's id in the harness.
    pub plugin: &'static str,
    /// The marketplace source used.
    pub marketplace: String,
    /// The version the harness reports, when it reports one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The commands run, in order. Empty when the plugin was already enabled.
    pub steps: Vec<Step>,
    /// Things that need attention.
    pub warnings: Vec<String>,
}

/// Runs `acs plugin`.
#[must_use]
pub fn run(args: &PluginArgs) -> ExitCode {
    match &args.command {
        PluginCommand::Install(install_args) => {
            finish(install_args.output.json, execute(install_args))
        }
    }
}

fn execute(args: &InstallArgs) -> Result<ExitCode, CliError> {
    let report = install(args)?;
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

/// Installs the plugin of `args.harness`.
///
/// # Errors
///
/// `unknown_harness` and `unsupported_harness` for a harness this command does not cover,
/// `acs_not_on_path`, `harness_not_found` when its command line is missing, `not_a_repo` for a
/// repository scope outside a git work tree. Failing steps are reported, not returned.
pub fn install(args: &InstallArgs) -> Result<InstallReport, CliError> {
    let harness = HARNESSES
        .iter()
        .find(|h| h.id == args.harness)
        .ok_or_else(|| {
            let known: Vec<&str> = HARNESSES.iter().map(|h| h.id).collect();
            CliError::new(
                "unknown_harness",
                format!(
                    "unknown harness `{}`; known: {}",
                    args.harness,
                    known.join(", ")
                ),
            )
        })?;
    match harness.id {
        "claude-code" => {
            require_acs_on_path(harness)?;
            let claude = find_harness_cli(harness)?;
            let dir = working_dir(args.scope)?;
            Ok(install_claude_code(&claude, &dir, args))
        }
        _ => Err(CliError::new(
            "unsupported_harness",
            format!(
                "`acs plugin install` does not cover {} yet. Install it by hand: {}",
                harness.name,
                harness.install.join(" && ")
            ),
        )),
    }
}

fn require_acs_on_path(harness: &Harness) -> Result<(), CliError> {
    match find_on_path("acs") {
        Some(_) => Ok(()),
        None => Err(CliError::new(
            "acs_not_on_path",
            format!(
                "`acs` is not on PATH, and without it the {} plugin is no use: {}. Put acs on \
                 PATH, then run this again.",
                harness.name, harness.without_acs
            ),
        )),
    }
}

fn find_harness_cli(harness: &Harness) -> Result<PathBuf, CliError> {
    harness
        .binaries
        .iter()
        .find_map(|binary| find_spawnable_on_path(binary))
        .ok_or_else(|| {
            CliError::new(
                "harness_not_found",
                format!(
                    "{} is not installed here: `{}` is not on PATH.",
                    harness.name,
                    harness.binaries.join("` or `")
                ),
            )
        })
}

/// Where the harness runs: the work tree root for a repository scope, because that is where it
/// writes its settings file.
fn working_dir(scope: Scope) -> Result<PathBuf, CliError> {
    let cwd = std::env::current_dir()
        .map_err(|err| CliError::new("cwd", format!("cannot read the current directory: {err}")))?;
    if scope == Scope::User {
        return Ok(cwd);
    }
    let layout = acs_git::discover(&cwd).map_err(|err| {
        CliError::new(
            "not_a_repo",
            format!(
                "scope `{}` needs a git repository: {err}. Run this inside one, or pass `--scope \
                 user`.",
                scope.as_str()
            ),
        )
    })?;
    Ok(layout.work_tree)
}

// ---------------------------------------------------------------------------------------------
// Claude Code
// ---------------------------------------------------------------------------------------------

/// The plugin as `claude plugin list --json` shows it from one directory.
#[derive(Debug, PartialEq, Eq)]
enum Listed {
    /// Enabled here at the scope asked for.
    Enabled {
        /// The installed version.
        version: Option<String>,
    },
    /// Not enabled here at that scope.
    Absent,
    /// The output could not be read.
    Unreadable,
}

/// Reads the listing. It also holds the plugin's entries for other projects, but `enabled` is
/// worked out for the directory `claude` ran in.
fn parse_listing(output: &[u8], scope: Scope) -> Listed {
    let Ok(serde_json::Value::Array(entries)) = serde_json::from_slice(output) else {
        return Listed::Unreadable;
    };
    entries
        .iter()
        .find(|entry| {
            entry["id"] == CLAUDE_CODE_PLUGIN
                && entry["enabled"] == true
                && entry["scope"] == scope.as_str()
        })
        .map_or(Listed::Absent, |entry| Listed::Enabled {
            version: entry["version"].as_str().map(str::to_owned),
        })
}

fn claude_command(claude: &Path, dir: &Path) -> Command {
    let mut command = Command::new(claude);
    command
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

fn list_claude_plugins(claude: &Path, dir: &Path, scope: Scope) -> Listed {
    claude_command(claude, dir)
        .args(["plugin", "list", "--json"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map_or(Listed::Unreadable, |out| parse_listing(&out.stdout, scope))
}

/// Runs `claude <args>` and records it as a step.
fn claude_step(claude: &Path, dir: &Path, args: &[&str]) -> Step {
    let line = format!("claude {}", args.join(" "));
    match claude_command(claude, dir).args(args).output() {
        Ok(out) if out.status.success() => Step {
            command: line,
            ok: true,
            output: None,
        },
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let stdout = String::from_utf8_lossy(&out.stdout);
            let text = if stderr.trim().is_empty() {
                stdout
            } else {
                stderr
            };
            Step {
                command: line,
                ok: false,
                output: Some(text.trim().to_owned()),
            }
        }
        Err(err) => Step {
            command: line,
            ok: false,
            output: Some(format!("could not start {}: {err}", claude.display())),
        },
    }
}

fn install_claude_code(claude: &Path, dir: &Path, args: &InstallArgs) -> InstallReport {
    let scope = args.scope.as_str();
    let mut report = InstallReport {
        ok: false,
        changed: false,
        harness: "claude-code",
        scope: args.scope,
        plugin: CLAUDE_CODE_PLUGIN,
        marketplace: args.marketplace.clone(),
        version: None,
        steps: Vec::new(),
        warnings: Vec::new(),
    };
    if args.scope == Scope::User {
        report.warnings.push(
            "user scope gates every git repository you open in Claude Code, with the built-in \
             guards where a repository has no .acs/policy. Prefer the default, project scope, \
             unless you want that."
                .to_owned(),
        );
    }
    if let Listed::Enabled { version } = list_claude_plugins(claude, dir, args.scope) {
        report.ok = true;
        report.version = version;
        return report;
    }
    let marketplace = ["plugin", "marketplace", "add", &args.marketplace];
    let plugin = ["plugin", "install", CLAUDE_CODE_PLUGIN];
    for command in [marketplace.as_slice(), plugin.as_slice()] {
        let step = claude_step(claude, dir, &[command, &["--scope", scope]].concat());
        let failed = !step.ok;
        report.steps.push(step);
        if failed {
            return report;
        }
    }
    report.changed = true;
    match list_claude_plugins(claude, dir, args.scope) {
        Listed::Enabled { version } => {
            report.ok = true;
            report.version = version;
        }
        Listed::Absent => report.warnings.push(format!(
            "Claude Code does not list {CLAUDE_CODE_PLUGIN} as enabled at {scope} scope. Check \
             `claude plugin list`."
        )),
        Listed::Unreadable => {
            report.ok = true;
            report.warnings.push(
                "the install commands succeeded, but `claude plugin list --json` could not be \
                 read to confirm it."
                    .to_owned(),
            );
        }
    }
    report
}

// ---------------------------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------------------------

/// The human view, at most 80 columns wide.
#[must_use]
pub fn render(report: &InstallReport) -> String {
    let mut out = format!("acs plugin install {}\n", report.harness);
    if !report.steps.is_empty() {
        out.push('\n');
    }
    for step in &report.steps {
        let tag = if step.ok { "done" } else { "FAILED" };
        out.push_str(&wrap(&step.command, &format!("  {tag:<7}"), "         "));
        if let Some(output) = &step.output {
            out.push_str(&wrap(output, "         ", "         "));
        }
    }
    for warning in &report.warnings {
        out.push('\n');
        out.push_str(&wrap(&format!("WARNING: {warning}"), "!! ", "   "));
    }
    if report.ok {
        out.push('\n');
        out.push_str(&outcome(report));
    }
    out
}

fn outcome(report: &InstallReport) -> String {
    let version = report
        .version
        .as_deref()
        .map_or_else(String::new, |v| format!("version {v}, "));
    let scope = report.scope.as_str();
    let mut text = if report.changed {
        format!(
            "The ACS plugin is installed and enabled ({version}{scope} scope). Claude Code loads \
             it at the next session start or on /reload-plugins.\n"
        )
    } else {
        format!("The ACS plugin is already enabled ({version}{scope} scope). Nothing to change.\n")
    };
    if report.changed && report.scope == Scope::Project {
        text.push_str("Commit .claude/settings.json so teammates get it too.\n");
    }
    wrap(&text, "", "")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_listing_counts_only_the_scope_asked_for_when_enabled_here() {
        let listing = br#"[
            {"id":"other@market","version":"9.9.9","scope":"project","enabled":true},
            {"id":"acs@acs-plugins","version":"0.1.0","scope":"project","enabled":false,
             "projectPath":"/somewhere/else"},
            {"id":"acs@acs-plugins","version":"0.1.0","scope":"user","enabled":true}
        ]"#;
        assert_eq!(parse_listing(listing, Scope::Project), Listed::Absent);
        assert_eq!(
            parse_listing(listing, Scope::User),
            Listed::Enabled {
                version: Some("0.1.0".to_owned())
            }
        );
    }

    #[test]
    fn output_that_is_not_a_listing_is_unreadable() {
        assert_eq!(parse_listing(b"", Scope::Project), Listed::Unreadable);
        assert_eq!(parse_listing(b"{}", Scope::Project), Listed::Unreadable);
        assert_eq!(parse_listing(b"[]", Scope::Project), Listed::Absent);
    }
}
