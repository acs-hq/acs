//! The canonical skills, the plugin that ships them, and the release they point at.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(rel: &str) -> String {
    fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|err| panic!("{rel}: {err}"))
}

fn canonical_skills() -> Vec<String> {
    fs::read_dir(repo_root().join("skills"))
        .unwrap()
        .map(|entry| entry.unwrap())
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect()
}

#[test]
fn both_skills_exist() {
    let mut skills = canonical_skills();
    skills.sort();
    assert_eq!(skills, ["acs-setup", "acs-usage"]);
}

#[test]
fn both_plugins_ship_exact_copies() {
    for plugin in ["claude-code", "antigravity"] {
        for skill in canonical_skills() {
            assert_eq!(
                read(&format!("skills/{skill}/SKILL.md")),
                read(&format!("integrations/{plugin}/skills/{skill}/SKILL.md")),
                "{skill}: copy skills/{skill}/SKILL.md over the {plugin} plugin's copy"
            );
        }
    }
}

#[test]
fn every_skill_has_the_front_matter_harnesses_load() {
    for skill in canonical_skills() {
        let text = read(&format!("skills/{skill}/SKILL.md"));
        let front = text
            .strip_prefix("---\n")
            .and_then(|rest| rest.split_once("\n---\n"))
            .unwrap_or_else(|| panic!("{skill}: no front matter"))
            .0;
        assert!(
            front.lines().any(|l| l == format!("name: {skill}")),
            "{skill}: {front}"
        );
        assert!(
            front.lines().any(|l| l.starts_with("description: ")),
            "{skill}: {front}"
        );
    }
}

#[test]
fn the_usage_skill_names_every_mcp_tool() {
    let text = read("skills/acs-usage/SKILL.md");
    for tool in [
        "acs_status",
        "acs_intent",
        "acs_claim",
        "acs_release",
        "acs_why",
    ] {
        assert!(
            text.contains(&format!("mcp__plugin_acs_acs__{tool}")),
            "{tool}"
        );
        // Antigravity names plugin MCP tools `mcp_<plugin>_<server>_<tool>`.
        assert!(text.contains(&format!("mcp_acs_acs_{tool}")), "{tool}");
    }
}

/// `dist` names the installers after the package, not the binary.
#[test]
fn the_setup_skill_points_at_the_installers_dist_builds() {
    let text = read("skills/acs-setup/SKILL.md");
    let package = env!("CARGO_PKG_NAME");
    for installer in ["sh", "ps1"] {
        assert!(
            text.contains(&format!("{package}-installer.{installer}")),
            "{installer}"
        );
    }
}

#[test]
fn the_setup_skill_stays_clear_of_user_scope_and_commits_before_the_gate() {
    let text = read("skills/acs-setup/SKILL.md");
    assert!(text.contains("acs plugin install <harness id>\n"));
    for id in ["claude-code", "antigravity"] {
        assert!(text.contains(&format!("| `{id}` |")), "{id}");
    }
    assert!(text.contains("Commit before the gate starts"));
    let commit = text.find("## 5. Commit").unwrap();
    let tell = text.find("## 6. Tell your pilot").unwrap();
    assert!(
        commit < tell,
        "the summary reports the commit, so it comes after"
    );
}

#[test]
fn the_plugin_version_is_the_release_version() {
    for path in [
        "integrations/claude-code/.claude-plugin/plugin.json",
        "integrations/antigravity/plugin.json",
    ] {
        let manifest: serde_json::Value = serde_json::from_str(&read(path)).unwrap();
        assert_eq!(manifest["version"], env!("CARGO_PKG_VERSION"), "{path}");
    }
}

#[test]
fn the_scoop_template_is_valid_and_names_the_archives_dist_builds() {
    let text = read("integrations/scoop/acs.json.in");
    let manifest: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(manifest["bin"], "acs.exe");
    let package = env!("CARGO_PKG_NAME");
    for (arch, target) in [
        ("64bit", "x86_64-pc-windows-msvc"),
        ("arm64", "aarch64-pc-windows-msvc"),
    ] {
        let url = manifest["architecture"][arch]["url"].as_str().unwrap();
        assert!(url.ends_with(&format!("/{package}-{target}.zip")), "{url}");
    }
    let dist = read("dist-workspace.toml");
    for target in ["x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc"] {
        assert!(dist.contains(target), "{target}");
    }
}

#[test]
fn the_marketplace_offers_the_plugin_directory() {
    let marketplace: serde_json::Value =
        serde_json::from_str(&read(".claude-plugin/marketplace.json")).unwrap();
    let plugin = &marketplace["plugins"][0];
    assert_eq!(plugin["name"], "acs");
    let source = plugin["source"].as_str().unwrap();
    assert!(
        repo_root()
            .join(source)
            .join(".claude-plugin/plugin.json")
            .is_file(),
        "{source}"
    );
}
