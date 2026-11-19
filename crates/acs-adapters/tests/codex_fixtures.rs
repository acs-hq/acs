//! Replays every payload captured from a real Codex CLI 0.170.0 run on Windows
//! (`tests/fixtures/codex/`, paths anonymized) through the adapter.

use std::fs;
use std::path::PathBuf;

use acs_adapters::{Adapter, codex::Codex};
use acs_core::{Phase, ToolEvent, ToolKind};

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/codex")
}

fn read(name: &str) -> Vec<u8> {
    fs::read(dir().join(format!("{name}.json"))).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn parse(name: &str) -> ToolEvent {
    Codex
        .parse(None, &read(name))
        .unwrap_or_else(|e| panic!("{name}: {e}"))
}

const CWD: &str = r"C:\Users\dev\project";

/// Events the plugin does not register: the adapter refuses them.
const UNSUPPORTED: &[&str] = &["permission_request.mcp", "stop"];

/// Fixture, phase, tool name, kind, paths, command.
type Case = (
    &'static str,
    Phase,
    &'static str,
    ToolKind,
    &'static [&'static str],
    Option<&'static str>,
);

#[test]
fn tool_fixtures_parse_as_expected() {
    let cases: &[Case] = &[
        (
            "pre_tool_use.bash",
            Phase::PreTool,
            "PowerShell",
            ToolKind::Shell,
            &[],
            Some("Set-Content -NoNewline c.txt 'new'"),
        ),
        (
            "post_tool_use.bash",
            Phase::PostTool,
            "PowerShell",
            ToolKind::Shell,
            &[],
            Some("Set-Content -NoNewline c.txt 'new'"),
        ),
        (
            "pre_tool_use.apply_patch.update",
            Phase::PreTool,
            "apply_patch",
            ToolKind::Edit,
            &["a.txt"],
            None,
        ),
        (
            "post_tool_use.apply_patch.update",
            Phase::PostTool,
            "apply_patch",
            ToolKind::Edit,
            &["a.txt"],
            None,
        ),
        (
            "pre_tool_use.apply_patch.add",
            Phase::PreTool,
            "apply_patch",
            ToolKind::Write,
            &["c.txt"],
            None,
        ),
        (
            "pre_tool_use.apply_patch.delete",
            Phase::PreTool,
            "apply_patch",
            ToolKind::Delete,
            &["b.txt"],
            None,
        ),
        (
            "pre_tool_use.apply_patch.move",
            Phase::PreTool,
            "apply_patch",
            ToolKind::Move,
            &["d.txt", "e.txt"],
            None,
        ),
        (
            "pre_tool_use.mcp",
            Phase::PreTool,
            "mcp__probe__probe_ping",
            ToolKind::Other,
            &[],
            None,
        ),
        (
            "post_tool_use.mcp",
            Phase::PostTool,
            "mcp__probe__probe_ping",
            ToolKind::Other,
            &[],
            None,
        ),
    ];
    for (file, phase, tool, kind, paths, command) in cases {
        let event = parse(file);
        assert_eq!(event.harness, "codex", "{file}");
        assert_eq!(event.phase, *phase, "{file}");
        assert_eq!(event.cwd, CWD, "{file}");
        let t = event.tool.as_ref().expect("tool");
        assert_eq!((t.name.as_str(), t.kind), (*tool, *kind), "{file}");
        assert_eq!(event.paths, *paths, "{file}");
        assert_eq!(event.command.as_deref(), *command, "{file}");
        assert_eq!(event.result.is_some(), *phase == Phase::PostTool, "{file}");
        assert!(event.raw["tool_use_id"].is_string(), "{file}");
    }
    let result = parse("post_tool_use.apply_patch.update")
        .result
        .expect("result");
    assert_eq!((result.success, result.exit_code), (true, Some(0)));
}

#[test]
fn session_and_prompt_fixtures() {
    let start = parse("session_start");
    assert_eq!(start.phase, Phase::SessionStart);
    assert_eq!(start.raw["source"], "startup");
    let prompt = parse("user_prompt_submit");
    assert_eq!(prompt.phase, Phase::PromptSubmit);
    assert!(
        prompt
            .prompt
            .as_deref()
            .unwrap_or("")
            .starts_with("Do these steps")
    );
    let end = parse("session_end");
    assert_eq!(end.phase, Phase::SessionEnd);
    assert_eq!(start.harness_session_id, end.harness_session_id);
}

#[test]
fn every_fixture_parses_or_is_listed_as_unsupported() {
    let mut seen = 0;
    for entry in fs::read_dir(dir()).expect("fixture dir") {
        let path = entry.expect("entry").path();
        let name = path.file_stem().and_then(|s| s.to_str()).expect("name");
        // MCP captures are replayed by `acs-mcp`'s binding tests.
        if name.starts_with("mcp_") {
            continue;
        }
        let bytes = fs::read(&path).expect("read");
        let parsed = Codex.parse(None, &bytes);
        if UNSUPPORTED.contains(&name) {
            let err = parsed.expect_err(name);
            assert!(err.harness_session_id.is_some(), "{name}");
        } else {
            let event = parsed.unwrap_or_else(|e| panic!("{name}: {e}"));
            let raw = event.raw.to_string();
            assert!(!raw.contains("rollout") && raw.len() < 1_000, "{raw}");
        }
        seen += 1;
    }
    assert_eq!(seen, 14);
}
