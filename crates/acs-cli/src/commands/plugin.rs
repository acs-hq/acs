//! `acs plugin install <harness>`: installs the ACS plugin into an agent harness.
//!
//! Keyed by harness name, so a harness joins by getting one arm in [`install`]: Claude Code
//! (decision 0021), Antigravity (decision 0024) and Codex (decision 0047).
//!
//! `acs` must resolve on `PATH` first: the plugin's hooks run `acs`, and without it they either
//! check nothing (Claude Code) or deny every tool call (Antigravity).
//!
//! The default scope is **project**, not the harness's own default of user: the hook gates every
//! git repository it runs in, using the built-in guards where there is no `.acs/policy`, so a
//! user-scope plugin changes how agents behave in repositories that never adopted ACS (decision
//! 0021). Running it again changes nothing.
//!
//! None of the three needs its command line on `PATH`, because people who use only a harness's
//! desktop app have none (issue 83, decision 0052). Claude Code's plugin is installed with
//! `claude plugin ...` when `claude` is there; without it, ACS writes the two settings entries
//! itself (project and local scope) and says what is left to do in the desktop app.
//!
//! Antigravity has no marketplace entry for ACS, so its plugin is built into the binary and
//! written to the workspace plugin folder `.agents/plugins/acs/`, which Antigravity loads at the
//! next session start. That folder is the project scope; the other scopes are not offered. `agy`
//! only validates the folder, and is skipped when it is missing.
//!
//! Codex has no plugin that the command line can install, so ACS adds a block of hooks and an
//! MCP server to Codex's `config.toml`: the project's `.codex/config.toml`, or the user's
//! `$CODEX_HOME/config.toml` with `--scope user` (`codex_config`). Codex has no local scope.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use clap::{Args, Subcommand, ValueEnum};
use serde::Serialize;

use super::claude_settings;
use super::codex_config;
use super::init::harness::{CLAUDE_CODE_PLUGIN, HARNESSES, Harness};
use super::support::{
    CliError, OutputArgs, find_on_path, find_spawnable_on_path, finish, print_json, wrap,
};

/// The marketplace the plugin comes from (this repository, `.claude-plugin/marketplace.json`).
pub const DEFAULT_MARKETPLACE: &str = "acs-hq/acs";

/// Where `acs plugin install antigravity` puts the plugin, relative to the work tree root.
pub const ANTIGRAVITY_PLUGIN_DIR: &str = ".agents/plugins/acs";

/// The Antigravity plugin's files, built in from `integrations/antigravity/`.
const ANTIGRAVITY_PLUGIN_FILES: &[(&str, &str)] = &[
    (
        "plugin.json",
        include_str!("../../../../integrations/antigravity/plugin.json"),
    ),
    (
        "assets/logo.svg",
        include_str!("../../../../integrations/antigravity/assets/logo.svg"),
    ),
    (
        "hooks.json",
        include_str!("../../../../integrations/antigravity/hooks.json"),
    ),
    (
        "mcp_config.json",
        include_str!("../../../../integrations/antigravity/mcp_config.json"),
    ),
    (
        "rules/acs.md",
        include_str!("../../../../integrations/antigravity/rules/acs.md"),
    ),
    (
        "skills/acs-setup/SKILL.md",
        include_str!("../../../../integrations/antigravity/skills/acs-setup/SKILL.md"),
    ),
    (
        "skills/acs-usage/SKILL.md",
        include_str!("../../../../integrations/antigravity/skills/acs-usage/SKILL.md"),
    ),
];

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
                  --scope user\n  acs plugin install claude-code --marketplace ./my-checkout\n  \
                  acs plugin install antigravity\n  acs plugin install codex\n\n\
                  Safe to run again: a plugin that is already enabled is left alone."
)]
pub struct InstallArgs {
    /// The harness, by adapter name (`claude-code`, `antigravity`, `codex`).
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
    /// What is left to do by hand, when acs could not finish the install itself (Claude Code
    /// without `claude` on `PATH`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_finish: Option<String>,
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
/// `acs_not_on_path`, `harness_not_found` for Claude Code at user scope without `claude` on
/// `PATH`, `not_a_repo` for a repository scope outside a git work tree. Failing steps are
/// reported, not returned.
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
            match find_harness_cli(harness) {
                Ok(claude) => {
                    let dir = working_dir(args.scope)?;
                    Ok(install_claude_code(&claude, &dir, args))
                }
                Err(err) if args.scope == Scope::User => Err(CliError::new(
                    err.code,
                    format!(
                        "{} acs does not edit your user settings itself. Install the plugin from \
                         the Claude desktop app instead: {DESKTOP_STEPS}, for your user account. \
                         Or use `--scope project` or `--scope local`, which acs can write \
                         without `claude`.",
                        err.message
                    ),
                )),
                Err(_) => {
                    let dir = working_dir(args.scope)?;
                    Ok(enable_claude_code_in_settings(&dir, args))
                }
            }
        }
        "antigravity" => {
            require_acs_on_path(harness)?;
            if args.scope != Scope::Project {
                return Err(CliError::new(
                    "unsupported_scope",
                    format!(
                        "Antigravity's ACS plugin is installed for this repository only (`--scope \
                         project`, in {ANTIGRAVITY_PLUGIN_DIR}/). For every repository, copy that \
                         folder and run `agy plugin install <folder>` yourself."
                    ),
                ));
            }
            let agy = find_harness_cli(harness).ok();
            let dir = working_dir(Scope::Project)?;
            Ok(install_antigravity(agy.as_deref(), &dir, args))
        }
        "codex" => {
            require_acs_on_path(harness)?;
            let file = match args.scope {
                Scope::Project => codex_config::project_file(&working_dir(Scope::Project)?),
                Scope::User => codex_config::user_file().ok_or_else(|| {
                    CliError::new(
                        "no_home",
                        "cannot find Codex's home folder: set CODEX_HOME or HOME.",
                    )
                })?,
                Scope::Local => {
                    return Err(CliError::new(
                        "unsupported_scope",
                        "Codex has no local scope: use `--scope project` (.codex/config.toml, \
                         shared) or `--scope user` (your Codex config.toml).",
                    ));
                }
            };
            Ok(install_codex(&file, args))
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
        to_finish: None,
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

/// How to install the plugin in the Claude desktop app's Code tab, as a clause.
const DESKTOP_STEPS: &str = "open the repository in the Code tab, click + next to the prompt \
                             box, choose Plugins, then Manage plugins, add the marketplace \
                             acs-hq/acs if acs is not listed, and install acs";

/// The settings file a repository scope writes, relative to the work tree root.
fn claude_settings_file(scope: Scope) -> &'static str {
    if scope == Scope::Local {
        ".claude/settings.local.json"
    } else {
        ".claude/settings.json"
    }
}

/// The `extraKnownMarketplaces` source for `--marketplace` as JSON text, in the shape and key
/// order Claude Code writes: a git URL, a local folder (made absolute), or a GitHub `owner/repo`.
fn marketplace_source(marketplace: &str, cwd: &Path) -> String {
    let quote = |s: &str| serde_json::to_string(s).unwrap_or_else(|_| format!("\"{s}\""));
    let looks_like_path = marketplace.starts_with(['.', '/', '\\', '~'])
        || marketplace.chars().nth(1) == Some(':')
        || cwd.join(marketplace).is_dir();
    if marketplace.contains("://") || marketplace.starts_with("git@") {
        format!(r#"{{"source": "git", "url": {}}}"#, quote(marketplace))
    } else if looks_like_path || marketplace.matches('/').count() != 1 {
        let path =
            std::path::absolute(cwd.join(marketplace)).unwrap_or_else(|_| cwd.join(marketplace));
        format!(
            r#"{{"source": "directory", "path": {}}}"#,
            quote(&path.to_string_lossy())
        )
    } else {
        format!(r#"{{"source": "github", "repo": {}}}"#, quote(marketplace))
    }
}

/// Claude Code without `claude` on `PATH` (issue 83): writes the plugin's two entries into the
/// project or local settings file, which the CLI and the desktop app both read, and says what is
/// left to do.
fn enable_claude_code_in_settings(dir: &Path, args: &InstallArgs) -> InstallReport {
    let rel = claude_settings_file(args.scope);
    let mut report = InstallReport {
        ok: false,
        changed: false,
        harness: "claude-code",
        scope: args.scope,
        plugin: CLAUDE_CODE_PLUGIN,
        marketplace: args.marketplace.clone(),
        version: None,
        steps: Vec::new(),
        warnings: vec![format!(
            "`claude` is not on PATH, so acs wrote the plugin into {rel} itself instead of \
             running `claude plugin install`."
        )],
        to_finish: Some(if args.scope == Scope::Local {
            "Claude Code fetches the plugin when a session starts in this repository and the \
             folder is trusted, in the CLI and in the desktop app's Code tab. Keep \
             .claude/settings.local.json out of git."
                .to_owned()
        } else {
            format!(
                "Claude Code does not download a plugin that only the shared settings enable. \
                 Finish it once on this machine in the Claude desktop app: {DESKTOP_STEPS}, for \
                 this project. Or install the Claude Code CLI and run this command again."
            )
        }),
    };
    let file = dir.join(rel);
    let existing = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => {
            report.steps.push(Step {
                command: format!("read {rel}"),
                ok: false,
                output: Some(err.to_string()),
            });
            return report;
        }
    };
    let cwd = std::env::current_dir().unwrap_or_else(|_| dir.to_path_buf());
    let source = format!(
        r#"{{"source": {}}}"#,
        marketplace_source(&args.marketplace, &cwd)
    );
    let entries = [
        ("extraKnownMarketplaces", "acs-plugins", source.as_str()),
        ("enabledPlugins", CLAUDE_CODE_PLUGIN, "true"),
    ];
    let line = format!("add {CLAUDE_CODE_PLUGIN} and its marketplace to {rel}");
    let new = match claude_settings::set_entries(&existing, &entries) {
        Ok(None) => {
            report.ok = true;
            return report;
        }
        Ok(Some(new)) => new,
        Err(err) => {
            report.steps.push(Step {
                command: line,
                ok: false,
                output: Some(format!("{rel}: {err}")),
            });
            return report;
        }
    };
    let written = file
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&file, new));
    report.ok = written.is_ok();
    report.changed = written.is_ok();
    report.steps.push(Step {
        command: line,
        ok: written.is_ok(),
        output: written.err().map(|err| err.to_string()),
    });
    report
}

// ---------------------------------------------------------------------------------------------
// Antigravity
// ---------------------------------------------------------------------------------------------

/// The `version` of the built-in Antigravity plugin manifest.
fn antigravity_plugin_version() -> Option<String> {
    let manifest = ANTIGRAVITY_PLUGIN_FILES
        .iter()
        .find(|(name, _)| *name == "plugin.json")?
        .1;
    serde_json::from_str::<serde_json::Value>(manifest)
        .ok()?
        .get("version")?
        .as_str()
        .map(str::to_owned)
}

/// Writes the built-in plugin into `<dir>/.agents/plugins/acs/` and has `agy` validate it, when
/// `agy` is on `PATH`. Files that already hold the built-in content are left alone; other files in
/// the folder are kept.
fn install_antigravity(agy: Option<&Path>, dir: &Path, args: &InstallArgs) -> InstallReport {
    let mut report = InstallReport {
        ok: false,
        changed: false,
        harness: "antigravity",
        scope: Scope::Project,
        plugin: "acs",
        marketplace: "built into acs".to_owned(),
        version: antigravity_plugin_version(),
        steps: Vec::new(),
        warnings: Vec::new(),
        to_finish: None,
    };
    if args.marketplace != DEFAULT_MARKETPLACE {
        report.warnings.push(
            "--marketplace is ignored for Antigravity: its plugin is built into acs.".to_owned(),
        );
    }
    let target = dir.join(ANTIGRAVITY_PLUGIN_DIR);
    let stale: Vec<&(&str, &str)> = ANTIGRAVITY_PLUGIN_FILES
        .iter()
        .filter(|(name, content)| {
            std::fs::read(target.join(name)).ok().as_deref() != Some(content.as_bytes())
        })
        .collect();
    if stale.is_empty() {
        report.ok = true;
        return report;
    }

    let written = stale.iter().try_for_each(|(name, content)| {
        let path = target.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, content)
    });
    let step = Step {
        command: format!(
            "write {} file(s) into {ANTIGRAVITY_PLUGIN_DIR}/",
            stale.len()
        ),
        ok: written.is_ok(),
        output: written.err().map(|err| err.to_string()),
    };
    let failed = !step.ok;
    report.steps.push(step);
    if failed {
        return report;
    }
    report.changed = true;

    let Some(agy) = agy else {
        report.ok = true;
        report.warnings.push(
            "`agy` is not on PATH, so the plugin was not validated. Antigravity loads it from \
             the folder all the same."
                .to_owned(),
        );
        return report;
    };
    let line = format!("agy plugin validate {ANTIGRAVITY_PLUGIN_DIR}");
    let step = match Command::new(agy)
        .args(["plugin", "validate"])
        .arg(&target)
        .current_dir(dir)
        .stdin(Stdio::null())
        .output()
    {
        Ok(out) if out.status.success() => Step {
            command: line,
            ok: true,
            output: None,
        },
        Ok(out) => Step {
            command: line,
            ok: false,
            output: Some(String::from_utf8_lossy(&out.stdout).trim().to_owned()),
        },
        Err(err) => Step {
            command: line,
            ok: false,
            output: Some(format!("could not start {}: {err}", agy.display())),
        },
    };
    report.ok = step.ok;
    report.steps.push(step);
    report
}

// ---------------------------------------------------------------------------------------------
// Codex
// ---------------------------------------------------------------------------------------------

/// Adds the ACS block to the Codex configuration file `file`, creating the file and its folder
/// when they do not exist. A file that already holds the current block is left alone.
fn install_codex(file: &Path, args: &InstallArgs) -> InstallReport {
    let mut report = InstallReport {
        ok: false,
        changed: false,
        harness: "codex",
        scope: args.scope,
        plugin: "acs",
        marketplace: "built into acs".to_owned(),
        version: None,
        steps: Vec::new(),
        warnings: Vec::new(),
        to_finish: None,
    };
    if args.marketplace != DEFAULT_MARKETPLACE {
        report.warnings.push(
            "--marketplace is ignored for Codex: its hooks and MCP server are built into acs."
                .to_owned(),
        );
    }
    if args.scope == Scope::User {
        report.warnings.push(
            "user scope gates every git repository you open in Codex, with the built-in guards \
             where a repository has no .acs/policy. Prefer the default, project scope, unless \
             you want that."
                .to_owned(),
        );
    }
    if find_spawnable_on_path("codex").is_none() {
        report.warnings.push(
            "`codex` is not on PATH here; the block is written anyway and takes effect where \
             Codex runs."
                .to_owned(),
        );
    }
    let existing = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => {
            report.steps.push(Step {
                command: format!("read {}", file.display()),
                ok: false,
                output: Some(err.to_string()),
            });
            return report;
        }
    };
    let line = format!("add the ACS block to {}", file.display());
    let new = match codex_config::add_block(&existing) {
        Ok(None) => {
            report.ok = true;
            return report;
        }
        Ok(Some(new)) => new,
        Err(err) => {
            report.steps.push(Step {
                command: line,
                ok: false,
                output: Some(err),
            });
            return report;
        }
    };
    let written = file
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(file, new));
    report.ok = written.is_ok();
    report.changed = written.is_ok();
    report.steps.push(Step {
        command: line,
        ok: written.is_ok(),
        output: written.err().map(|err| err.to_string()),
    });
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
    if report.harness == "antigravity" {
        let text = if report.changed {
            format!(
                "The ACS plugin is installed in {ANTIGRAVITY_PLUGIN_DIR}/ ({version}{scope} \
                 scope). Antigravity loads it at the next session start.\nCommit \
                 {ANTIGRAVITY_PLUGIN_DIR} so teammates get it too.\n"
            )
        } else {
            format!(
                "The ACS plugin is already installed in {ANTIGRAVITY_PLUGIN_DIR}/ \
                 ({version}{scope} scope). Nothing to change.\n"
            )
        };
        return wrap(&text, "", "");
    }
    if report.harness == "codex" {
        let place = if report.scope == Scope::User {
            "your Codex config.toml"
        } else {
            codex_config::PROJECT_FILE
        };
        let mut text = if report.changed {
            format!(
                "The ACS hooks and MCP server are in {place} ({scope} scope). Codex loads them \
                 at the next session start.\n"
            )
        } else {
            format!("The ACS hooks and MCP server are already in {place}. Nothing to change.\n")
        };
        if report.changed {
            text.push_str(
                "Codex runs them only once you trust them: start `codex` here, accept the \
                 project as trusted if asked, then review and trust the ACS hooks in /hooks.\n\
                 The Codex app reads the same file: open this folder there and trust the \
                 project when asked. If the app does not offer to review the hooks, review them \
                 once in the CLI's /hooks.\n",
            );
            if report.scope == Scope::Project {
                text.push_str("Commit .codex/config.toml so teammates get it too.\n");
            }
        }
        return wrap(&text, "", "");
    }
    let mut text = match (&report.to_finish, report.changed) {
        (Some(to_finish), changed) => {
            let file = claude_settings_file(report.scope);
            let state = if changed { "now" } else { "already" };
            format!("The ACS plugin is {state} enabled in {file} ({scope} scope).\n{to_finish}\n")
        }
        (None, true) => format!(
            "The ACS plugin is installed and enabled ({version}{scope} scope). Claude Code loads \
             it at the next session start or on /reload-plugins.\n"
        ),
        (None, false) => format!(
            "The ACS plugin is already enabled ({version}{scope} scope). Nothing to change.\n"
        ),
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
    fn marketplace_sources_take_claude_codes_shapes() {
        use serde_json::{Value, json};
        let cwd = std::env::temp_dir();
        let source =
            |m: &str| -> Value { serde_json::from_str(&marketplace_source(m, &cwd)).unwrap() };
        assert_eq!(
            marketplace_source("acs-hq/acs", &cwd),
            r#"{"source": "github", "repo": "acs-hq/acs"}"#
        );
        assert_eq!(
            source("https://example.com/acs.git"),
            json!({"source": "git", "url": "https://example.com/acs.git"})
        );
        assert_eq!(source("git@example.com:acs/acs.git")["source"], "git");
        let local = source("./my-checkout");
        assert_eq!(local["source"], "directory");
        let path = PathBuf::from(local["path"].as_str().unwrap());
        assert!(path.is_absolute(), "{path:?}");
        assert!(path.ends_with("my-checkout"), "{path:?}");
    }

    #[test]
    fn output_that_is_not_a_listing_is_unreadable() {
        assert_eq!(parse_listing(b"", Scope::Project), Listed::Unreadable);
        assert_eq!(parse_listing(b"{}", Scope::Project), Listed::Unreadable);
        assert_eq!(parse_listing(b"[]", Scope::Project), Listed::Absent);
    }
}
