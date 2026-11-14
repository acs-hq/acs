//! The default policy files `acs init` writes into `.acs/policy/`.
//!
//! Every template is valid policy as written (`config.toml` sets `spec` and writes out the
//! `[features]` switches at their defaults; the other two are entirely comments), and the
//! values are generated from the built-in defaults, so they cannot drift from what the engine
//! applies. Existing files are never touched.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use acs_core::{DEFAULT_SEALED, Feature, Features};
use acs_policy::{DEFAULT_BAN, DEFAULT_PROTECTED_BRANCHES};
use serde::Serialize;

/// What happened to one policy file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileAction {
    /// The file did not exist and was written.
    Created,
    /// The file exists; it was left as it is.
    Exists,
    /// The file held the untouched default and was replaced (`territory.toml` with
    /// `acs init --yes`).
    Replaced,
}

/// One policy file and what `init` did with it.
#[derive(Debug, Clone, Serialize)]
pub struct PolicyFileReport {
    /// File name inside `.acs/policy/`.
    pub file: &'static str,
    /// What happened.
    pub action: FileAction,
}

/// The `(file name, contents)` of each template, in the order they are written. `branch` is the
/// repository's main branch when it is not `main` (decision 0040): `config.toml` then sets it as
/// `default_target_branch` and `guards.toml` adds it to the default `protected_branches`.
#[must_use]
pub fn templates(branch: Option<&str>) -> [(&'static str, String); 3] {
    let branch = branch.filter(|b| *b != DEFAULT_TARGET_BRANCH);
    [
        ("config.toml", config_template(branch)),
        ("guards.toml", guards_template(branch)),
        ("territory.toml", territory_template()),
    ]
}

/// The branch the built-in `default_target_branch` names.
pub const DEFAULT_TARGET_BRANCH: &str = "main";

/// Writes the policy directory and the files that are missing. `territory`, when given, is
/// written in place of the `territory.toml` template; `branch` is passed to [`templates`].
///
/// # Errors
///
/// An I/O error (for example `.acs` is a file, or the directory is read-only).
pub fn write_missing(
    policy_dir: &Path,
    territory: Option<&str>,
    branch: Option<&str>,
) -> io::Result<Vec<PolicyFileReport>> {
    fs::create_dir_all(policy_dir)?;
    let mut reports = Vec::new();
    for (file, template) in templates(branch) {
        let contents = match territory {
            Some(text) if file == "territory.toml" => text.to_owned(),
            _ => template,
        };
        let path = policy_dir.join(file);
        // `create_new` makes "never overwrite" atomic: a file that appears meanwhile is kept.
        let action = match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut handle) => {
                handle.write_all(contents.as_bytes())?;
                FileAction::Created
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => FileAction::Exists,
            Err(err) => return Err(err),
        };
        reports.push(PolicyFileReport { file, action });
    }
    Ok(reports)
}

fn quoted_list(items: &[&str], indent: &str) -> String {
    items
        .iter()
        .map(|item| format!("{indent}\"{item}\",\n"))
        .collect()
}

/// `text` as a TOML basic string (JSON's escapes are valid TOML).
fn toml_string(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| format!("\"{text}\""))
}

fn config_template(branch: Option<&str>) -> String {
    let target = match branch {
        Some(name) => format!(
            "# The branch work is integrated into. `acs init` found that it is not `main`.\n\
             default_target_branch = {}\n",
            toml_string(name)
        ),
        None => format!(
            "# The branch work is integrated into.\n# default_target_branch = \"{DEFAULT_TARGET_BRANCH}\"\n"
        ),
    };
    let defaults = Features::default();
    let features: String = Feature::ALL
        .into_iter()
        .map(|feature| {
            format!(
                "# {}.\n{} = {}\n",
                capitalized(feature.summary()),
                feature.key(),
                defaults.is_on(feature)
            )
        })
        .collect();
    format!(
        "\
# ACS repository policy, spec acs/0.1. Commit this file: it is the same for every clone.
# Everything except `spec` is optional; the commented values are the defaults.
spec = \"acs/0.1\"

{target}
# Seconds a claim stays live without a heartbeat.
# claim_ttl_secs = 900

# Seconds between background syncs of claims with the git remote.
# sync_interval_secs = 30

# [ledger]
# Store the text of user prompts in the session ledger.
# store_prompts = false

# Which parts of ACS run in this repository. Set one to false to switch it off (or run
# `acs features <name> on|off`). The values below are the defaults. Only people can change
# them: ACS's protection of its own files, hooks and state is always on.
[features]
{features}"
    )
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().collect::<String>() + chars.as_str()
    })
}

fn guards_template(branch: Option<&str>) -> String {
    let sealed = quoted_list(DEFAULT_SEALED, "#   ");
    let mut branches = DEFAULT_PROTECTED_BRANCHES
        .iter()
        .map(|b| format!("\"{b}\""))
        .collect::<Vec<_>>();
    let protected = match branch {
        Some(name) => {
            branches.push(toml_string(name));
            format!(
                "# Branches on which history rewrites and force pushes are denied. The defaults, plus\n\
                 # the main branch `acs init` found.\n\
                 protected_branches = [{}]\n",
                branches.join(", ")
            )
        }
        None => format!(
            "# Branches on which history rewrites and force pushes are denied.\n\
             # protected_branches = [{}]\n",
            branches.join(", ")
        ),
    };
    let ban = DEFAULT_BAN
        .iter()
        .map(|b| format!("\"{b}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "\
# ACS guards: paths and commands agents may never touch.
# Without this file (or a key in it) the built-in defaults below apply. Setting a key REPLACES
# its default, so copy the defaults you want to keep.

# Globs of paths no agent may write.
# sealed = [
{sealed}# ]

{protected}
# Ask before an agent writes or deletes outside the repository. Off by default: ACS guards the
# repository. Paths it cannot resolve, and paths outside that link back into the repository, are
# asked about either way; ACS's own state and harness settings are always denied.
# protect_outside_repo = false

# [destructive]
# Files a command may delete outside the seat's claims (0 denies any such delete).
# max_files_deleted_outside_claims = 0
# Files one command may delete before the human is asked.
# max_files_deleted_per_command = 50
# Command patterns that are always denied (`*` matches any run of characters).
# ban = [{ban}]
"
    )
}

/// The opening comment of `territory.toml`, shared by the template and proposals.
pub const TERRITORY_HEADER: &str = "\
# ACS territory: who may change what. Without zones the whole repository is `shared`: any
# agent may change a file after claiming it. When zones overlap, the most specific one wins.
# kinds: owned (only the owners' agents), shared (any agent after a claim), open (no claim
# needed), sealed (no agent).
";

/// Whether `text` is the `territory.toml` template as `acs init` writes it (line endings aside).
#[must_use]
pub fn is_territory_template(text: &str) -> bool {
    text.replace("\r\n", "\n") == territory_template()
}

/// The `territory.toml` written when nothing is proposed or the proposal is not taken.
#[must_use]
pub fn territory_template() -> String {
    TERRITORY_HEADER.to_owned()
        + "
# [[zone]]
# paths = [\"payments/**\"]
# kind = \"owned\"
# owners = [\"sara@example.com\"]

# [[zone]]
# paths = [\"docs/**\", \"scratch/**\"]
# kind = \"open\"

# Teams are referenced from `owners` as \"team:<name>\".
# [teams]
# platform = [\"omar@example.com\", \"lina@example.com\"]
"
}

#[cfg(test)]
mod tests {
    use super::*;
    use acs_policy::{LoadOptions, load};

    #[test]
    fn the_templates_are_valid_policy_equal_to_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let policy_dir = dir.path().join("policy");
        let reports = write_missing(&policy_dir, None, None).unwrap();
        assert!(reports.iter().all(|r| r.action == FileAction::Created));
        let loaded = load(&policy_dir, LoadOptions::default()).unwrap();
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
        let defaults = load(&dir.path().join("none"), LoadOptions::default()).unwrap();
        assert_eq!(loaded.policy.files(), defaults.policy.files());
    }

    #[test]
    fn uncommenting_the_guards_template_keeps_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let text = guards_template(None)
            .lines()
            .map(|l| {
                l.strip_prefix("# ")
                    .or_else(|| l.strip_prefix('#'))
                    .unwrap_or(l)
            })
            // Keep the prose comments out; only the TOML lines remain.
            .filter(|l| {
                l.starts_with("sealed")
                    || l.starts_with("protected")
                    || l.starts_with("protect_outside")
                    || l.starts_with("[destructive]")
                    || l.starts_with("max_files")
                    || l.starts_with("ban")
                    || l.starts_with("  \"")
                    || *l == "]"
            })
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(dir.path().join("guards.toml"), text).unwrap();
        let loaded = load(dir.path(), LoadOptions::default()).unwrap();
        let defaults = load(&dir.path().join("none"), LoadOptions::default()).unwrap();
        assert_eq!(loaded.policy.files(), defaults.policy.files());
    }

    #[test]
    fn another_main_branch_is_written_and_keeps_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        write_missing(dir.path(), None, Some("master")).unwrap();
        let loaded = load(dir.path(), LoadOptions::default()).unwrap();
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
        let files = loaded.policy.files();
        assert_eq!(files.config.default_target_branch, "master");
        let mut wanted: Vec<String> = DEFAULT_PROTECTED_BRANCHES
            .iter()
            .map(|b| (*b).to_owned())
            .collect();
        wanted.push("master".to_owned());
        assert_eq!(files.guards.protected_branches, wanted);
        // Everything else stays at the defaults.
        let defaults = load(&dir.path().join("none"), LoadOptions::default()).unwrap();
        assert_eq!(files.guards.sealed, defaults.policy.files().guards.sealed);
        assert_eq!(
            files.config.features,
            defaults.policy.files().config.features
        );
    }

    #[test]
    fn main_writes_the_plain_templates() {
        assert_eq!(templates(Some("main")), templates(None));
    }

    #[test]
    fn existing_files_are_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("config.toml"),
            "spec = \"acs/0.1\"\n# mine\n",
        )
        .unwrap();
        let reports = write_missing(dir.path(), None, None).unwrap();
        assert_eq!(reports[0].action, FileAction::Exists);
        assert_eq!(reports[1].action, FileAction::Created);
        assert!(
            fs::read_to_string(dir.path().join("config.toml"))
                .unwrap()
                .contains("# mine")
        );
    }
}
