//! The default policy files `acs init` writes into `.acs/policy/`.
//!
//! Every template is valid policy as written (`config.toml` sets only `spec`; the other two are
//! entirely comments), and the commented values are generated from the built-in defaults, so
//! they cannot drift from what the engine applies. Existing files are never touched.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use acs_core::DEFAULT_SEALED;
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
}

/// One policy file and what `init` did with it.
#[derive(Debug, Clone, Serialize)]
pub struct PolicyFileReport {
    /// File name inside `.acs/policy/`.
    pub file: &'static str,
    /// What happened.
    pub action: FileAction,
}

/// The `(file name, contents)` of each template, in the order they are written.
#[must_use]
pub fn templates() -> [(&'static str, String); 3] {
    [
        ("config.toml", config_template()),
        ("guards.toml", guards_template()),
        ("territory.toml", territory_template()),
    ]
}

/// Writes the policy directory and the files that are missing.
///
/// # Errors
///
/// An I/O error (for example `.acs` is a file, or the directory is read-only).
pub fn write_missing(policy_dir: &Path) -> io::Result<Vec<PolicyFileReport>> {
    fs::create_dir_all(policy_dir)?;
    let mut reports = Vec::new();
    for (file, contents) in templates() {
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

fn config_template() -> String {
    "\
# ACS repository policy, spec acs/0.1. Commit this file: it is the same for every clone.
# Everything except `spec` is optional; the commented values are the defaults.
spec = \"acs/0.1\"

# The branch work is integrated into.
# default_target_branch = \"main\"

# Seconds a claim stays live without a heartbeat.
# claim_ttl_secs = 900

# Seconds between background syncs of claims with the git remote.
# sync_interval_secs = 30

# [ledger]
# Store the text of user prompts in the session ledger.
# store_prompts = false
"
    .to_owned()
}

fn guards_template() -> String {
    let sealed = quoted_list(DEFAULT_SEALED, "#   ");
    let branches = DEFAULT_PROTECTED_BRANCHES
        .iter()
        .map(|b| format!("\"{b}\""))
        .collect::<Vec<_>>()
        .join(", ");
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

# Branches on which history rewrites and force pushes are denied.
# protected_branches = [{branches}]

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

fn territory_template() -> String {
    "\
# ACS territory: who may change what. Without zones the whole repository is `shared`: any
# agent may change a file after claiming it. When zones overlap, the most specific one wins.
# kinds: owned (only the owners' agents), shared (any agent after a claim), open (no claim
# needed), sealed (no agent).

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
    .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use acs_policy::{LoadOptions, load};

    #[test]
    fn the_templates_are_valid_policy_equal_to_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let policy_dir = dir.path().join("policy");
        let reports = write_missing(&policy_dir).unwrap();
        assert!(reports.iter().all(|r| r.action == FileAction::Created));
        let loaded = load(&policy_dir, LoadOptions::default()).unwrap();
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
        let defaults = load(&dir.path().join("none"), LoadOptions::default()).unwrap();
        assert_eq!(loaded.policy.files(), defaults.policy.files());
    }

    #[test]
    fn uncommenting_the_guards_template_keeps_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let text = guards_template()
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
    fn existing_files_are_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("config.toml"),
            "spec = \"acs/0.1\"\n# mine\n",
        )
        .unwrap();
        let reports = write_missing(dir.path()).unwrap();
        assert_eq!(reports[0].action, FileAction::Exists);
        assert_eq!(reports[1].action, FileAction::Created);
        assert!(
            fs::read_to_string(dir.path().join("config.toml"))
                .unwrap()
                .contains("# mine")
        );
    }
}
