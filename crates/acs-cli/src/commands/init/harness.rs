//! Detecting the agent harnesses a repository is used with, and what to run to install the ACS
//! plugin for each. `init` only **prints** the commands; `acs plugin install` runs them, for the
//! harnesses it covers; `doctor` reuses the detection and a best-effort check that the plugin is
//! installed.
//!
//! Sources: `docs/adapters/claude-code.md` section 10 and `docs/adapters/antigravity.md`
//! section 5.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::commands::support::find_on_path;

/// The ACS plugin's id in Claude Code: plugin `acs` of the marketplace `acs-plugins`.
pub const CLAUDE_CODE_PLUGIN: &str = "acs@acs-plugins";

/// A harness ACS has an adapter and a plugin for.
#[derive(Debug, Clone, Copy)]
pub struct Harness {
    /// The adapter name (`acs hook <id>`).
    pub id: &'static str,
    /// The product name.
    pub name: &'static str,
    /// Executables whose presence on `PATH` means the harness is installed.
    pub binaries: &'static [&'static str],
    /// Directories in the work tree whose presence means the repository uses the harness.
    pub dirs: &'static [&'static str],
    /// The commands that install the plugin, in order.
    pub install: &'static [&'static str],
    /// What happens when the plugin's hooks run without `acs` on `PATH`.
    pub without_acs: &'static str,
    /// Whether that makes every tool call fail (Antigravity fails closed).
    pub fails_closed: bool,
}

/// The supported harnesses.
pub const HARNESSES: &[Harness] = &[
    Harness {
        id: "claude-code",
        name: "Claude Code",
        binaries: &["claude"],
        dirs: &[".claude"],
        install: &["acs plugin install claude-code"],
        without_acs: "its hooks fail open: Claude Code shows a hook error and the tool call \
                      runs unchecked",
        fails_closed: false,
    },
    Harness {
        id: "antigravity",
        name: "Antigravity",
        binaries: &["agy", "antigravity"],
        dirs: &[".agents", ".agent"],
        install: &["acs plugin install antigravity"],
        without_acs: "its hooks fail CLOSED: Antigravity denies every tool call",
        fails_closed: true,
    },
];

/// A harness found on this machine or in this repository.
#[derive(Debug, Clone, Serialize)]
pub struct Detected {
    /// The adapter name.
    pub id: &'static str,
    /// The product name.
    pub name: &'static str,
    /// What was found (`claude on PATH`, `.claude/ directory`).
    pub found: Vec<String>,
    /// The commands that install the plugin.
    pub install: Vec<&'static str>,
    /// Whether a missing `acs` binary makes the harness deny every tool call.
    pub fails_closed_without_acs: bool,
    #[serde(skip)]
    pub(crate) harness: &'static Harness,
}

/// The harnesses found on `PATH` or as directories in `work_tree`.
#[must_use]
pub fn detect(work_tree: &Path) -> Vec<Detected> {
    HARNESSES
        .iter()
        .filter_map(|harness| {
            let mut found = Vec::new();
            for binary in harness.binaries {
                if find_on_path(binary).is_some() {
                    found.push(format!("`{binary}` on PATH"));
                }
            }
            for dir in harness.dirs {
                if work_tree.join(dir).is_dir() {
                    found.push(format!("`{dir}/` directory"));
                }
            }
            (!found.is_empty()).then(|| Detected {
                id: harness.id,
                name: harness.name,
                found,
                install: harness.install.to_vec(),
                fails_closed_without_acs: harness.fails_closed,
                harness,
            })
        })
        .collect()
}

/// Whether the ACS plugin is known to be installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginState {
    /// Found enabled (a settings file or a plugin directory says so).
    Installed,
    /// Not found where we can look cheaply and safely; it may still be installed (another
    /// scope, managed settings, a harness-internal registry).
    NotConfirmed,
}

/// Looks for the ACS plugin without running the harness: Claude Code's `enabledPlugins` key
/// `acs@acs-plugins` in the user, project and local settings files; Antigravity's plugin
/// directories (`<work tree>/.agents/plugins/acs`, `~/.gemini/config/plugins/acs`).
#[must_use]
pub fn plugin_state(harness: &Harness, work_tree: &Path) -> PluginState {
    let installed = match harness.id {
        "claude-code" => claude_code_plugin_enabled(work_tree),
        "antigravity" => antigravity_plugin_present(work_tree),
        _ => false,
    };
    if installed {
        PluginState::Installed
    } else {
        PluginState::NotConfirmed
    }
}

fn user_home() -> Option<PathBuf> {
    std::env::home_dir().filter(|p| !p.as_os_str().is_empty())
}

/// The project and local Claude Code settings files of a work tree, which the project and
/// local plugin scopes write.
#[must_use]
pub fn claude_code_project_settings(work_tree: &Path) -> [PathBuf; 2] {
    [
        work_tree.join(".claude/settings.json"),
        work_tree.join(".claude/settings.local.json"),
    ]
}

/// The user's Claude Code settings file (`$CLAUDE_CONFIG_DIR/settings.json`, else
/// `~/.claude/settings.json`).
#[must_use]
pub fn claude_code_user_settings() -> Option<PathBuf> {
    match std::env::var_os("CLAUDE_CONFIG_DIR").filter(|v| !v.is_empty()) {
        Some(dir) => Some(PathBuf::from(dir).join("settings.json")),
        None => user_home().map(|h| h.join(".claude/settings.json")),
    }
}

/// Whether the settings file enables the ACS plugin.
#[must_use]
pub fn settings_enable_acs(file: &Path) -> bool {
    std::fs::read(file)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|json| {
            json.get("enabledPlugins")?
                .get(CLAUDE_CODE_PLUGIN)?
                .as_bool()
        })
        == Some(true)
}

fn claude_code_plugin_enabled(work_tree: &Path) -> bool {
    let mut files = claude_code_project_settings(work_tree).to_vec();
    files.extend(claude_code_user_settings());
    files.iter().any(|file| settings_enable_acs(file))
}

fn antigravity_plugin_present(work_tree: &Path) -> bool {
    let mut dirs = vec![
        work_tree.join(".agents/plugins/acs"),
        work_tree.join(".agent/plugins/acs"),
    ];
    dirs.extend(user_home().map(|h| h.join(".gemini/config/plugins/acs")));
    dirs.iter().any(|dir| dir.is_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directories_in_the_work_tree_are_detected() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".agents")).unwrap();
        let found = detect(dir.path());
        assert!(
            found
                .iter()
                .any(|d| d.id == "antigravity" && d.fails_closed_without_acs)
        );
    }

    #[test]
    fn a_project_settings_file_confirms_the_claude_code_plugin() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".claude")).unwrap();
        std::fs::write(
            dir.path().join(".claude/settings.json"),
            r#"{"enabledPlugins":{"acs@acs-plugins":true}}"#,
        )
        .unwrap();
        assert!(claude_code_plugin_enabled(dir.path()));
    }
}
