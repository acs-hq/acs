//! Replays every recorded Antigravity payload in `tests/fixtures/antigravity/` through the
//! adapter: inputs are parsed, outputs are compared with what the adapter renders.

use std::fs;
use std::path::PathBuf;

use acs_adapters::{Adapter, antigravity::Antigravity};
use acs_core::{Decision, Outcome, Phase, ReasonCode, ToolEvent, ToolKind};
use serde_json::Value;

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/antigravity")
}

fn read(name: &str) -> Vec<u8> {
    fs::read(dir().join(format!("{name}.json"))).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn json(name: &str) -> Value {
    serde_json::from_slice(&read(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn parse(name: &str, phase: Option<Phase>) -> ToolEvent {
    Antigravity
        .parse(phase, &read(name))
        .unwrap_or_else(|e| panic!("{name}: {e}"))
}

struct Case {
    file: &'static str,
    phase: Phase,
    tool: &'static str,
    kind: ToolKind,
    paths: &'static [&'static str],
    command: Option<&'static str>,
    cwd: &'static str,
}

const SHELL: ToolKind = ToolKind::Shell;

#[test]
fn tool_fixtures_parse_as_expected() {
    let w = "/home/dev/proj";
    let cases = [
        Case {
            file: "pre_tool_run_command",
            phase: Phase::PreTool,
            tool: "run_command",
            kind: SHELL,
            paths: &[],
            command: Some("git status --short"),
            cwd: w,
        },
        Case {
            file: "pre_tool_run_command_legacy_cwd",
            phase: Phase::PreTool,
            tool: "run_command",
            kind: SHELL,
            paths: &[],
            command: Some("rm -rf build"),
            cwd: w,
        },
        Case {
            file: "pre_tool_run_command_windows",
            phase: Phase::PreTool,
            tool: "run_command (powershell)",
            kind: SHELL,
            paths: &[],
            command: Some("Remove-Item -Recurse -Force .\\build"),
            cwd: "C:\\Users\\dev\\proj",
        },
        Case {
            file: "pre_tool_write_file",
            phase: Phase::PreTool,
            tool: "write_file",
            kind: ToolKind::Write,
            paths: &["/home/dev/proj/src/lib.rs"],
            command: None,
            cwd: w,
        },
        Case {
            file: "pre_tool_write_to_file_legacy",
            phase: Phase::PreTool,
            tool: "write_to_file",
            kind: ToolKind::Write,
            paths: &["/home/dev/proj/src/lib.rs"],
            command: None,
            cwd: w,
        },
        Case {
            file: "pre_tool_edit_file",
            phase: Phase::PreTool,
            tool: "edit_file",
            kind: ToolKind::Edit,
            paths: &["/home/dev/proj/src/lib.rs"],
            command: None,
            cwd: w,
        },
        Case {
            file: "pre_tool_replace_file_content_legacy",
            phase: Phase::PreTool,
            tool: "replace_file_content",
            kind: ToolKind::Edit,
            paths: &["/home/dev/proj/src/lib.rs"],
            command: None,
            cwd: w,
        },
        Case {
            file: "pre_tool_multi_replace_file_content_legacy",
            phase: Phase::PreTool,
            tool: "multi_replace_file_content",
            kind: ToolKind::Edit,
            paths: &["/home/dev/proj/src/lib.rs"],
            command: None,
            cwd: w,
        },
        Case {
            file: "pre_tool_view_file_legacy",
            phase: Phase::PreTool,
            tool: "view_file",
            kind: ToolKind::Read,
            paths: &["/home/dev/proj/README.md"],
            command: None,
            cwd: w,
        },
        Case {
            file: "pre_tool_read_file",
            phase: Phase::PreTool,
            tool: "read_file",
            kind: ToolKind::Read,
            paths: &["/home/dev/proj/README.md"],
            command: None,
            cwd: w,
        },
        Case {
            file: "pre_tool_call_mcp_tool",
            phase: Phase::PreTool,
            tool: "call_mcp_tool",
            kind: ToolKind::Other,
            paths: &[],
            command: None,
            cwd: w,
        },
        Case {
            file: "post_tool_write_file",
            phase: Phase::PostTool,
            tool: "write_file",
            kind: ToolKind::Write,
            paths: &["/home/dev/proj/src/lib.rs"],
            command: None,
            cwd: w,
        },
        Case {
            file: "post_tool_run_command_error",
            phase: Phase::PostTool,
            tool: "run_command",
            kind: SHELL,
            paths: &[],
            command: Some("cargo test"),
            cwd: w,
        },
    ];
    for case in cases {
        // With the phase from the command line, and inferred from the payload alone.
        for given in [Some(case.phase), None] {
            let event = parse(case.file, given);
            assert_eq!(event.phase, case.phase, "{}", case.file);
            assert_eq!(event.harness, "antigravity");
            assert_eq!(
                event.harness_session_id,
                "3f2a9c1e-7b4d-4e0a-9d51-0c6b8e2a7f10"
            );
            let tool = event
                .tool
                .as_ref()
                .unwrap_or_else(|| panic!("{}", case.file));
            assert_eq!(
                (tool.name.as_str(), tool.kind),
                (case.tool, case.kind),
                "{}",
                case.file
            );
            assert_eq!(event.paths, case.paths, "{}", case.file);
            assert_eq!(event.command.as_deref(), case.command, "{}", case.file);
            assert_eq!(event.cwd, case.cwd, "{}", case.file);
            assert_eq!(event.raw, json(case.file));
        }
    }
}

#[test]
fn edit_fixtures_carry_old_and_new_text() {
    let new = parse("pre_tool_edit_file", Some(Phase::PreTool))
        .edit
        .unwrap();
    assert_eq!(
        (new.old_text.as_str(), new.new_text.as_str()),
        ("a + b", "a.wrapping_add(b)")
    );
    let legacy = parse("pre_tool_replace_file_content_legacy", Some(Phase::PreTool))
        .edit
        .unwrap();
    assert_eq!(legacy, new);
    // Two chunks: path-level only.
    assert!(
        parse(
            "pre_tool_multi_replace_file_content_legacy",
            Some(Phase::PreTool)
        )
        .edit
        .is_none()
    );
    assert!(
        parse("pre_tool_write_file", Some(Phase::PreTool))
            .edit
            .is_none()
    );
}

#[test]
fn post_tool_results_come_from_error() {
    let ok = parse("post_tool_write_file", None).result.unwrap();
    assert!(ok.success && ok.exit_code.is_none());
    let failed = parse("post_tool_run_command_error", None).result.unwrap();
    assert!(!failed.success && failed.exit_code.is_none());
    assert!(parse("pre_tool_write_file", None).result.is_none());
}

#[test]
fn invocation_and_stop_fixtures() {
    let first = parse("pre_invocation", Some(Phase::PromptSubmit));
    assert_eq!(first.phase, Phase::SessionStart);
    assert!(first.tool.is_none());
    // Without --phase an invocation payload is a prompt_submit (refined to session_start above
    // only when given a prompt phase and the numbers say first).
    assert_eq!(parse("pre_invocation", None).phase, Phase::SessionStart);
    assert_eq!(
        parse("post_invocation", Some(Phase::PromptSubmit)).phase,
        Phase::SessionStart
    );
    // `Stop` has no tool and cannot be inferred: ACS registers no Stop hook, but an explicit phase
    // still parses.
    assert!(Antigravity.parse(None, &read("stop")).is_err());
    assert_eq!(
        parse("stop", Some(Phase::SessionEnd)).phase,
        Phase::SessionEnd
    );
}

fn decision(outcome: Outcome, message: &str) -> Decision {
    Decision::new(outcome, ReasonCode::Ok, message)
}

fn rendered(phase: Phase, decision: &Decision) -> Value {
    let event = ToolEvent {
        phase,
        ..parse("pre_tool_run_command", Some(Phase::PreTool))
    };
    let out = Antigravity.render(Some(&event), decision);
    assert_eq!(out.exit_code, 0);
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("exactly one JSON object: {e}"))
}

#[test]
fn output_fixtures_are_what_the_adapter_renders() {
    let deny = json("out_pre_tool_deny");
    let message = deny["reason"].as_str().unwrap();
    assert_eq!(
        rendered(Phase::PreTool, &decision(Outcome::Deny, message)),
        deny
    );

    let ask = json("out_pre_tool_force_ask");
    let message = ask["reason"].as_str().unwrap();
    assert_eq!(
        rendered(Phase::PreTool, &decision(Outcome::Ask, message)),
        ask
    );

    // ACS allow is "no objection", never `allow`.
    let allow = rendered(Phase::PreTool, &decision(Outcome::Allow, ""));
    assert_eq!(allow, json("out_pre_tool_ask_defer"));
    assert_ne!(allow, json("out_pre_tool_allow"));

    let inject = json("out_pre_invocation_inject");
    let mut with_context = decision(Outcome::Allow, "");
    with_context.context.push(
        inject["injectSteps"][0]["ephemeralMessage"]
            .as_str()
            .unwrap()
            .to_owned(),
    );
    assert_eq!(rendered(Phase::SessionStart, &with_context), inject);
    assert_eq!(rendered(Phase::PromptSubmit, &with_context), inject);

    let empty = json("out_empty_object");
    assert_eq!(
        rendered(Phase::PromptSubmit, &decision(Outcome::Allow, "")),
        empty
    );
    assert_eq!(
        rendered(Phase::PostTool, &decision(Outcome::Allow, "")),
        empty
    );
    assert_eq!(
        rendered(Phase::SessionEnd, &decision(Outcome::Allow, "")),
        empty
    );
}

#[test]
fn fixtures_not_produced_by_the_adapter_are_still_valid_json() {
    // `allow` and the Stop continue answer are documented shapes ACS deliberately never emits;
    // the MCP request is the acs-mcp crate's input.
    for name in [
        "out_pre_tool_allow",
        "out_stop_continue",
        "mcp_tools_call_meta",
    ] {
        assert!(json(name).is_object(), "{name}");
    }
    assert_eq!(
        json("mcp_tools_call_meta")["params"]["_meta"]["antigravity.google/root_conversation_id"],
        "3f2a9c1e-7b4d-4e0a-9d51-0c6b8e2a7f10"
    );
}

#[test]
fn every_fixture_is_covered_by_a_test() {
    let covered = [
        "pre_tool_run_command",
        "pre_tool_run_command_legacy_cwd",
        "pre_tool_run_command_windows",
        "pre_tool_write_file",
        "pre_tool_write_to_file_legacy",
        "pre_tool_edit_file",
        "pre_tool_replace_file_content_legacy",
        "pre_tool_multi_replace_file_content_legacy",
        "pre_tool_view_file_legacy",
        "pre_tool_read_file",
        "pre_tool_call_mcp_tool",
        "post_tool_write_file",
        "post_tool_run_command_error",
        "pre_invocation",
        "post_invocation",
        "stop",
        "out_pre_tool_deny",
        "out_pre_tool_force_ask",
        "out_pre_tool_ask_defer",
        "out_pre_tool_allow",
        "out_pre_invocation_inject",
        "out_empty_object",
        "out_stop_continue",
        "mcp_tools_call_meta",
    ];
    let mut on_disk: Vec<String> = fs::read_dir(dir())
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter_map(|n| n.strip_suffix(".json").map(str::to_owned))
        .collect();
    on_disk.sort();
    let mut expected: Vec<String> = covered.iter().map(|s| (*s).to_owned()).collect();
    expected.sort();
    assert_eq!(on_disk, expected, "add new fixtures to the tests");
}

#[test]
fn the_plugin_hooks_run_this_adapter_with_known_phases() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../integrations/antigravity");
    let hooks: Value = serde_json::from_slice(&fs::read(path.join("hooks.json")).unwrap()).unwrap();
    let group = &hooks["acs"];
    assert!(
        group.get("Stop").is_none(),
        "no Stop hook: it fires at every turn end"
    );
    let mut seen = Vec::new();
    for (event, expected_phase) in [
        ("PreToolUse", "pre_tool"),
        ("PostToolUse", "post_tool"),
        ("PreInvocation", "prompt_submit"),
    ] {
        let entry = &group[event][0];
        // Tool events take matcher groups, the others a flat list.
        let handler = if event == "PreInvocation" {
            entry
        } else {
            &entry["hooks"][0]
        };
        if event != "PreInvocation" {
            assert_eq!(entry["matcher"], ".*");
        }
        assert_eq!(handler["type"], "command");
        assert_eq!(handler["timeout"], 10);
        let command = handler["command"].as_str().unwrap();
        assert_eq!(
            command,
            format!("acs hook antigravity --phase {expected_phase}")
        );
        seen.push(expected_phase);
    }
    assert_eq!(seen.len(), 3);
    let manifest: Value =
        serde_json::from_slice(&fs::read(path.join("plugin.json")).unwrap()).unwrap();
    assert_eq!(manifest["name"], "acs");
    let mcp: Value =
        serde_json::from_slice(&fs::read(path.join("mcp_config.json")).unwrap()).unwrap();
    assert_eq!(
        mcp["mcpServers"]["acs"]["args"],
        serde_json::json!(["mcp", "--harness", "antigravity"])
    );
    let rule = fs::read_to_string(path.join("rules/acs.md")).unwrap();
    assert!(rule.starts_with("---\ntrigger: always_on\n"));
    assert!(rule.contains("acs_status") && rule.contains("acs_intent"));
}
