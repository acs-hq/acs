//! WP7.2's adversarial suite: every command in `tests/fixtures/adversarial.txt` marked `deny` is
//! denied when an agent's shell tool runs it, and the `allow` controls keep working. Each is sent
//! through the built binary as a canonical `pre_tool` event (decision 0026).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use acs_core::{Decision, Outcome};
use common::{World, acs_bin};
use serde_json::json;

const FIXTURES: &str = include_str!("fixtures/adversarial.txt");

struct Case {
    line: usize,
    deny: bool,
    tool: &'static str,
    command: String,
}

fn cases() -> Vec<Case> {
    let mut out = Vec::new();
    for (index, raw) in FIXTURES.lines().enumerate() {
        let line = raw.trim_end();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (kind, command) = line
            .split_once(": ")
            .unwrap_or_else(|| panic!("line {}: no `kind: command`", index + 1));
        let (deny, tool) = match kind {
            "deny" => (true, "Bash"),
            "allow" => (false, "Bash"),
            "deny-ps" => (true, "PowerShell"),
            "allow-ps" => (false, "PowerShell"),
            other => panic!("line {}: unknown kind {other:?}", index + 1),
        };
        out.push(Case {
            line: index + 1,
            deny,
            tool,
            command: command.replace("\\n", "\n"),
        });
    }
    out
}

/// Sends one shell event with HOME and USERPROFILE pointing at `user_home`.
fn decide(world: &World, user_home: &Path, tool: &str, command: &str) -> Decision {
    let event = json!({
        "spec": "acs/0.1",
        "harness": "claude-code",
        "harness_session_id": "adversary",
        "phase": "pre_tool",
        "cwd": world.repo,
        "tool": {"name": tool, "kind": "shell"},
        "command": command,
    });
    let mut child = Command::new(acs_bin())
        .args(["hook", "--canonical", "pre_tool"])
        .current_dir(&world.repo)
        .env("ACS_HOME", &world.home)
        .env("HOME", user_home)
        .env("USERPROFILE", user_home)
        .env_remove("CLAUDE_CONFIG_DIR")
        .env("GIT_CONFIG_GLOBAL", &world.git_config)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(event.to_string().as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "acs failed: {out:?}");
    serde_json::from_slice(&out.stdout).unwrap()
}

/// Every file under `dir` with its size and modification time.
fn snapshot(dir: &Path, out: &mut Vec<(String, u64, Option<std::time::SystemTime>)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let meta = entry.metadata().unwrap();
        if meta.is_dir() {
            // A directory's own time moves for reasons unrelated to ACS; its files are listed.
            out.push((path.display().to_string(), 0, None));
            snapshot(&path, out);
        } else {
            out.push((path.display().to_string(), meta.len(), meta.modified().ok()));
        }
    }
    out.sort();
}

/// Decision 0038: the shapes the hook lets through as help really only print help. Each allowed
/// `--help` / `-h` form of the fixture runs on the built binary: exit 0, usage on stdout, and
/// neither the repository nor the ACS home changes.
#[test]
fn the_help_forms_the_hook_allows_only_print_help() {
    let world = World::new();
    let mut ran = 0;
    for case in cases().iter().filter(|c| !c.deny) {
        let Some(rest) = case.command.strip_prefix("acs ") else {
            continue;
        };
        let words: Vec<&str> = rest.split_whitespace().collect();
        if !matches!(words.last(), Some(&("--help" | "-h"))) {
            continue;
        }
        let (mut before, mut after) = (Vec::new(), Vec::new());
        snapshot(&world.repo, &mut before);
        snapshot(&world.home, &mut before);
        let out = Command::new(acs_bin())
            .args(&words)
            .current_dir(&world.repo)
            .env("ACS_HOME", &world.home)
            .env("GIT_CONFIG_GLOBAL", &world.git_config)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(out.status.success(), "{}: {out:?}", case.command);
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("Usage:"), "{}: {stdout}", case.command);
        snapshot(&world.repo, &mut after);
        snapshot(&world.home, &mut after);
        assert_eq!(before, after, "{} changed state", case.command);
        ran += 1;
    }
    assert!(ran >= 6, "the help forms of the fixture shrank ({ran})");
}

#[test]
fn every_adversarial_command_is_denied_and_the_controls_are_not() {
    let world = World::new();
    let user_home = world.tmp.path().join("user-home");
    for dir in [".claude/plugins", ".gemini/config"] {
        fs::create_dir_all(user_home.join(dir)).unwrap();
    }
    fs::write(user_home.join(".claude/settings.json"), "{}").unwrap();

    let cases = cases();
    assert!(
        cases.iter().filter(|c| c.deny).count() >= 60,
        "the suite shrank"
    );
    let mut failures = Vec::new();
    for case in &cases {
        let decision = decide(&world, &user_home, case.tool, &case.command);
        let denied = decision.outcome == Outcome::Deny;
        if denied != case.deny {
            failures.push(format!(
                "line {}: {:?} ({}) -> {:?} {}: {}",
                case.line,
                case.command,
                case.tool,
                decision.outcome,
                decision.reason_code,
                decision.message
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} cases failed:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}
