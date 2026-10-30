//! The Claude Code adapter.
//!
//! Payload and output shapes are specified, with sources and capture provenance, in
//! `docs/adapters/claude-code.md`; fixtures are in `tests/fixtures/claude-code/`.
//!
//! - **Input** (stdin): one JSON object whose `hook_event_name` selects the phase
//!   (`SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `PostToolUseFailure`,
//!   `SessionEnd`). Any other event, or a payload without a session id, a working directory or
//!   the fields its event needs, is an [`AdapterError`] carrying whatever could be recovered.
//! - **Output**: allow is an empty stdout (never `permissionDecision: "allow"`, which would skip
//!   the user's own permission prompt) or an `additionalContext` document; deny is the JSON deny
//!   document on stdout, the same message on stderr and exit code 2 (so a malformed document still
//!   blocks); ask is `permissionDecision: "ask"`; `queue` is treated as deny.

use acs_core::{
    Decision, EditInfo, Outcome, Phase, SpecVersion, Tool, ToolEvent, ToolKind, ToolResult,
};
use serde_json::{Map, Value, json};

use super::{Adapter, AdapterError, HookOutput};

/// Claude Code caps hook `additionalContext` strings at 10,000 characters and replaces longer
/// ones with a file path and a preview, which the model is not asked to read. Stay below it.
const CONTEXT_LIMIT: usize = 10_000;

/// Marker appended when text is cut to [`CONTEXT_LIMIT`].
const TRUNCATED: &str = "\n[ACS: text shortened]";

/// The adapter registered as `claude-code`.
#[derive(Debug, Clone, Copy, Default)]
pub struct ClaudeCode;

impl Adapter for ClaudeCode {
    fn name(&self) -> &'static str {
        "claude-code"
    }

    fn parse(&self, phase: Option<Phase>, payload: &[u8]) -> Result<ToolEvent, AdapterError> {
        let value: Value = serde_json::from_slice(payload)
            .map_err(|err| AdapterError::new(format!("not JSON: {err}")))?;
        let Value::Object(map) = &value else {
            return Err(AdapterError::new("payload is not a JSON object"));
        };
        let fail = |message: String| AdapterError::recovering(message, payload);

        let session_id =
            text(map, "session_id").ok_or_else(|| fail("payload has no session_id".to_owned()))?;
        let cwd = text(map, "cwd").ok_or_else(|| fail("payload has no cwd".to_owned()))?;
        let event_name = text(map, "hook_event_name");
        let phase = match event_name {
            Some("SessionStart") => Phase::SessionStart,
            Some("UserPromptSubmit") => Phase::PromptSubmit,
            Some("PreToolUse") => Phase::PreTool,
            Some("PostToolUse" | "PostToolUseFailure") => Phase::PostTool,
            Some("SessionEnd") => Phase::SessionEnd,
            Some(other) => return Err(fail(format!("unsupported hook event {other:?}"))),
            None => phase.ok_or_else(|| fail("payload has no hook_event_name".to_owned()))?,
        };

        let mut event = ToolEvent {
            spec: SpecVersion,
            harness: "claude-code".to_owned(),
            harness_session_id: session_id.to_owned(),
            phase,
            cwd: cwd.to_owned(),
            tool: None,
            paths: Vec::new(),
            edit: None,
            command: None,
            result: None,
            prompt: None,
            raw: raw_summary(map),
        };

        match phase {
            Phase::PreTool | Phase::PostTool => {
                let name = text(map, "tool_name")
                    .ok_or_else(|| fail("tool event has no tool_name".to_owned()))?;
                let empty = Map::new();
                let input = map
                    .get("tool_input")
                    .and_then(Value::as_object)
                    .unwrap_or(&empty);
                let shape = classify(name, input).map_err(fail)?;
                event.tool = Some(Tool {
                    name: name.to_owned(),
                    kind: shape.kind,
                });
                event.paths = shape.paths;
                event.edit = shape.edit;
                event.command = shape.command;
                if phase == Phase::PostTool {
                    event.result = Some(post_result(
                        event_name == Some("PostToolUseFailure"),
                        &event,
                        map,
                    ));
                }
            }
            Phase::PromptSubmit => {
                event.prompt = text(map, "prompt").map(str::to_owned);
            }
            Phase::SessionStart | Phase::SessionEnd => {}
        }
        Ok(event)
    }

    fn render(&self, event: Option<&ToolEvent>, decision: &Decision) -> HookOutput {
        let Some(event) = event else {
            // Unparseable payload: the pipeline allows. Anything stronger cannot be placed in
            // an event we do not know, so block through the exit code only.
            return match decision.outcome {
                Outcome::Allow => HookOutput::default(),
                Outcome::Deny | Outcome::Ask | Outcome::Queue => blocking_stderr(decision),
            };
        };
        match event.phase {
            Phase::PreTool => render_pre_tool(decision),
            Phase::PostTool => {
                let failed = event.result.as_ref().is_some_and(|r| !r.success);
                let name = if failed {
                    "PostToolUseFailure"
                } else {
                    "PostToolUse"
                };
                context_output(name, &everything(decision))
            }
            Phase::SessionStart => context_output("SessionStart", &everything(decision)),
            Phase::PromptSubmit => context_output("UserPromptSubmit", &everything(decision)),
            // SessionEnd output is discarded by Claude Code.
            Phase::SessionEnd => HookOutput::default(),
        }
    }
}

/// How a tool call maps onto the canonical event.
struct Shape {
    kind: ToolKind,
    paths: Vec<String>,
    edit: Option<EditInfo>,
    command: Option<String>,
}

fn text<'a>(map: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    map.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

fn shape(kind: ToolKind) -> Shape {
    Shape {
        kind,
        paths: Vec::new(),
        edit: None,
        command: None,
    }
}

/// Maps a Claude Code tool name and input to kind, paths, edit text and command.
///
/// Returns a message when a tool that changes files lacks the field naming its target: the
/// payload shape changed, and the pipeline should allow with `unknown_payload` and say so rather
/// than gate on nothing.
fn classify(name: &str, input: &Map<String, Value>) -> Result<Shape, String> {
    let required = |key: &str| -> Result<String, String> {
        text(input, key)
            .map(str::to_owned)
            .ok_or_else(|| format!("{name} input has no {key}"))
    };
    let optional_path = |key: &str| text(input, key).map(str::to_owned).into_iter().collect();
    Ok(match name {
        "Write" => Shape {
            paths: vec![required("file_path")?],
            ..shape(ToolKind::Write)
        },
        "Edit" => Shape {
            paths: vec![required("file_path")?],
            edit: match (text_any(input, "old_string"), text_any(input, "new_string")) {
                (Some(old), Some(new)) => Some(EditInfo {
                    old_text: old.to_owned(),
                    new_text: new.to_owned(),
                }),
                _ => None,
            },
            ..shape(ToolKind::Edit)
        },
        // Legacy: several edit pairs in one call; no single `EditInfo`.
        "MultiEdit" => Shape {
            paths: vec![required("file_path")?],
            ..shape(ToolKind::Edit)
        },
        "NotebookEdit" => Shape {
            paths: vec![required("notebook_path")?],
            ..shape(ToolKind::Edit)
        },
        "Bash" | "PowerShell" => Shape {
            command: Some(required("command")?),
            ..shape(ToolKind::Shell)
        },
        "Monitor" => match text(input, "command") {
            Some(command) => Shape {
                command: Some(command.to_owned()),
                ..shape(ToolKind::Shell)
            },
            // A WebSocket watch cannot change files.
            None => shape(ToolKind::Other),
        },
        "Read" => Shape {
            paths: optional_path("file_path"),
            ..shape(ToolKind::Read)
        },
        "NotebookRead" => Shape {
            paths: optional_path("notebook_path"),
            ..shape(ToolKind::Read)
        },
        "Glob" | "Grep" => Shape {
            paths: optional_path("path"),
            ..shape(ToolKind::Read)
        },
        // WebFetch, WebSearch, Agent, Workflow, Skill, task and plan tools, `mcp__*` and every
        // tool this version does not know: none of the built-ins that remain can change files
        // (subagents' own calls arrive as separate events).
        _ => shape(ToolKind::Other),
    })
}

/// A string field that may legitimately be empty (an edit's `new_string` deletes text).
fn text_any<'a>(map: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    map.get(key).and_then(Value::as_str)
}

/// The result of a finished tool call.
///
/// `PostToolUse` for a shell tool means exit code 0 (a non-zero exit fires `PostToolUseFailure`);
/// the failure event carries `Exit code N` as the first line of `error`.
fn post_result(failure: bool, event: &ToolEvent, map: &Map<String, Value>) -> ToolResult {
    let is_shell = event
        .tool
        .as_ref()
        .is_some_and(|tool| tool.kind == ToolKind::Shell);
    if failure {
        let exit_code = map
            .get("error")
            .and_then(Value::as_str)
            .and_then(|error| error.lines().next())
            .and_then(|line| line.strip_prefix("Exit code "))
            .and_then(|code| code.trim().parse::<i32>().ok());
        return ToolResult {
            success: false,
            exit_code,
        };
    }
    let interrupted = map
        .get("tool_response")
        .and_then(|r| r.get("interrupted"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    ToolResult {
        success: !interrupted,
        exit_code: (is_shell && !interrupted).then_some(0),
    }
}

/// The small fields of the payload worth keeping in `raw`: identifiers and modes, never tool
/// input, tool output, prompts or paths to transcripts (some are large, some are private).
fn raw_summary(map: &Map<String, Value>) -> Value {
    const KEEP: &[&str] = &[
        "hook_event_name",
        "tool_use_id",
        "prompt_id",
        "permission_mode",
        "agent_id",
        "agent_type",
        "source",
        "reason",
        "model",
        "mcp_server",
        "is_interrupt",
        "duration_ms",
    ];
    let kept: Map<String, Value> = KEEP
        .iter()
        .filter_map(|key| map.get(*key).map(|v| ((*key).to_owned(), v.clone())))
        .collect();
    Value::Object(kept)
}

// ---------------------------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------------------------

/// Message, context and warnings as one block of plain statements.
fn everything(decision: &Decision) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if !decision.message.is_empty() {
        parts.push(&decision.message);
    }
    parts.extend(decision.context.iter().map(String::as_str));
    parts.extend(decision.warnings.iter().map(String::as_str));
    parts.retain(|part| !part.trim().is_empty());
    cap(parts.join("\n"))
}

/// Keeps `text` within [`CONTEXT_LIMIT`] characters, cutting on a character boundary.
fn cap(text: String) -> String {
    if text.chars().count() <= CONTEXT_LIMIT {
        return text;
    }
    let keep = CONTEXT_LIMIT - TRUNCATED.chars().count();
    let mut cut: String = text.chars().take(keep).collect();
    cut.push_str(TRUNCATED);
    cut
}

fn json_output(value: &Value) -> Vec<u8> {
    // A `Value` always serializes; the fallback only avoids a panic path.
    serde_json::to_vec(value).unwrap_or_else(|_| b"{}".to_vec())
}

/// `additionalContext` for `event_name`, or nothing at all when there is no text.
fn context_output(event_name: &str, text: &str) -> HookOutput {
    if text.is_empty() {
        return HookOutput::default();
    }
    HookOutput {
        stdout: json_output(&json!({
            "hookSpecificOutput": {
                "hookEventName": event_name,
                "additionalContext": text,
            }
        })),
        stderr: Vec::new(),
        exit_code: 0,
    }
}

/// The reason shown to the model for a deny or ask: the message, then any context or warnings.
fn reason(decision: &Decision) -> String {
    let text = everything(decision);
    if decision.message.is_empty() {
        let fallback = format!(
            "ACS blocked this tool call ({}). Call acs_status for details.",
            serde_json::to_value(decision.reason_code)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_else(|| "unknown".to_owned())
        );
        if text.is_empty() {
            return fallback;
        }
        return cap(format!("{fallback}\n{text}"));
    }
    text
}

fn blocking_stderr(decision: &Decision) -> HookOutput {
    HookOutput {
        stdout: Vec::new(),
        stderr: reason(decision).into_bytes(),
        exit_code: 2,
    }
}

fn render_pre_tool(decision: &Decision) -> HookOutput {
    match decision.outcome {
        Outcome::Allow => context_output("PreToolUse", &everything(decision)),
        Outcome::Deny | Outcome::Queue => {
            let reason = reason(decision);
            HookOutput {
                stdout: json_output(&json!({
                    "hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "permissionDecision": "deny",
                        "permissionDecisionReason": reason,
                    }
                })),
                stderr: reason.into_bytes(),
                exit_code: 2,
            }
        }
        Outcome::Ask => HookOutput {
            stdout: json_output(&json!({
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": "ask",
                    "permissionDecisionReason": reason(decision),
                }
            })),
            stderr: Vec::new(),
            exit_code: 0,
        },
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use acs_core::ReasonCode;

    use super::*;

    fn fixture_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/claude-code")
    }

    fn fixture(name: &str) -> Vec<u8> {
        let path = fixture_dir().join(format!("{name}.json"));
        std::fs::read(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()))
    }

    fn parse(name: &str) -> ToolEvent {
        ClaudeCode
            .parse(None, &fixture(name))
            .unwrap_or_else(|err| panic!("{name}: {err}"))
    }

    fn tool(event: &ToolEvent) -> (&str, ToolKind) {
        let tool = event.tool.as_ref().expect("tool");
        (tool.name.as_str(), tool.kind)
    }

    const PROJECT: &str = "/home/dev/project";
    const SESSION: &str = "5b2e6f0a-3c1d-4e7a-9a58-0d2f6c1b7e34";

    #[test]
    fn every_fixture_parses_or_is_a_documented_unsupported_event() {
        let mut parsed = 0;
        for entry in std::fs::read_dir(fixture_dir()).expect("fixture dir") {
            let path = entry.expect("entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let bytes = std::fs::read(&path).expect("read");
            let event = ClaudeCode
                .parse(None, &bytes)
                .unwrap_or_else(|err| panic!("{}: {err}", path.display()));
            assert_eq!(event.harness, "claude-code");
            assert_eq!(event.harness_session_id, SESSION, "{}", path.display());
            assert!(!event.cwd.is_empty());
            // Nothing large or private ends up in `raw`.
            let raw = event.raw.to_string();
            assert!(!raw.contains("transcript") && raw.len() < 2_000, "{raw}");
            parsed += 1;
        }
        assert!(parsed >= 30, "only {parsed} fixtures");
    }

    #[test]
    fn write_and_edit() {
        let event = parse("pre_tool_use.write");
        assert_eq!(event.phase, Phase::PreTool);
        assert_eq!(tool(&event), ("Write", ToolKind::Write));
        assert_eq!(event.paths, vec![format!("{PROJECT}/hello.txt")]);
        assert!(event.edit.is_none() && event.command.is_none() && event.result.is_none());

        let event = parse("pre_tool_use.edit");
        assert_eq!(tool(&event), ("Edit", ToolKind::Edit));
        assert_eq!(event.paths, vec![format!("{PROJECT}/hello.txt")]);
        let edit = event.edit.expect("edit");
        assert_eq!(
            (edit.old_text.as_str(), edit.new_text.as_str()),
            ("hi", "hello")
        );
        assert_eq!(event.raw["permission_mode"], "acceptEdits");
        assert_eq!(event.raw["tool_use_id"], "toolu_01R4RR2wUTQ3t5GHg8EXv7Jv");
    }

    #[test]
    fn an_edit_that_deletes_text_keeps_its_empty_new_string() {
        let payload =
            br#"{"session_id":"s","cwd":"/r","hook_event_name":"PreToolUse","tool_name":"Edit",
            "tool_input":{"file_path":"/r/a","old_string":"x","new_string":""}}"#;
        let event = ClaudeCode.parse(None, payload).expect("parse");
        assert_eq!(event.edit.expect("edit").new_text, "");
    }

    #[test]
    fn notebook_edit_names_the_notebook() {
        for name in [
            "pre_tool_use.notebook_edit",
            "pre_tool_use.notebook_edit.insert",
        ] {
            let event = parse(name);
            assert_eq!(tool(&event), ("NotebookEdit", ToolKind::Edit));
            assert_eq!(event.paths.len(), 1);
            assert!(event.paths[0].ends_with("/nb.ipynb"));
            assert!(event.edit.is_none());
        }
    }

    #[test]
    fn shell_tools_carry_the_command() {
        let event = parse("pre_tool_use.bash.rm");
        assert_eq!(tool(&event), ("Bash", ToolKind::Shell));
        assert_eq!(event.command.as_deref(), Some("rm moved.txt"));
        assert!(event.paths.is_empty());

        let event = parse("pre_tool_use.powershell");
        assert_eq!(tool(&event), ("PowerShell", ToolKind::Shell));
        assert!(event.command.expect("command").starts_with("Remove-Item"));

        let event = parse("pre_tool_use.monitor");
        assert_eq!(tool(&event), ("Monitor", ToolKind::Shell));
        assert_eq!(event.command.as_deref(), Some("tail -f build.log"));

        let ws =
            br#"{"session_id":"s","cwd":"/r","hook_event_name":"PreToolUse","tool_name":"Monitor",
            "tool_input":{"description":"d","timeout_ms":1,"ws":{"url":"wss://x"}}}"#;
        let event = ClaudeCode.parse(None, ws).expect("parse");
        assert_eq!(tool(&event).1, ToolKind::Other);
    }

    #[test]
    fn read_search_and_web_tools() {
        let event = parse("pre_tool_use.read");
        assert_eq!(tool(&event), ("Read", ToolKind::Read));
        assert_eq!(event.paths, vec![format!("{PROJECT}/hello.txt")]);
        for (name, tool_name) in [("pre_tool_use.glob", "Glob"), ("pre_tool_use.grep", "Grep")] {
            let event = parse(name);
            assert_eq!(tool(&event), (tool_name, ToolKind::Read));
            assert_eq!(event.paths.len(), 1);
        }
        let event = parse("pre_tool_use.webfetch");
        assert_eq!(tool(&event), ("WebFetch", ToolKind::Other));
        assert!(event.paths.is_empty());
    }

    #[test]
    fn agent_skill_and_mcp_tools_are_other_and_subagent_ids_stay_in_raw() {
        for name in [
            "pre_tool_use.agent",
            "pre_tool_use.skill",
            "pre_tool_use.mcp",
            "pre_tool_use.mcp_plugin",
        ] {
            let event = parse(name);
            assert_eq!(tool(&event).1, ToolKind::Other, "{name}");
            assert!(event.paths.is_empty() && event.command.is_none(), "{name}");
        }
        let event = parse("pre_tool_use.mcp_plugin");
        assert_eq!(tool(&event).0, "mcp__plugin_acs-demo_envdump__whoami");
        assert_eq!(event.raw["mcp_server"]["source"], "plugin");

        let event = parse("pre_tool_use.bash.subagent");
        assert_eq!(
            event.harness_session_id, SESSION,
            "subagents share the seat"
        );
        assert_eq!(event.raw["agent_type"], "general-purpose");
        assert!(event.raw["agent_id"].is_string());
    }

    #[test]
    fn windows_paths_are_passed_through_raw() {
        let event = parse("pre_tool_use.write.windows");
        assert_eq!(event.paths, vec!["C:\\project\\src\\index.ts"]);
        assert_eq!(event.cwd, "C:\\project");
    }

    #[test]
    fn post_tool_results() {
        let event = parse("post_tool_use.write");
        assert_eq!(event.phase, Phase::PostTool);
        assert_eq!(
            event.result,
            Some(ToolResult {
                success: true,
                exit_code: None
            })
        );
        assert_eq!(event.paths, vec![format!("{PROJECT}/hello.txt")]);

        let event = parse("post_tool_use.bash");
        assert_eq!(
            event.result,
            Some(ToolResult {
                success: true,
                exit_code: Some(0)
            })
        );
        assert_eq!(event.command.as_deref(), Some("ls"));

        let event = parse("post_tool_use_failure.bash");
        assert_eq!(event.phase, Phase::PostTool);
        assert_eq!(
            event.result,
            Some(ToolResult {
                success: false,
                exit_code: Some(1)
            })
        );

        let event = parse("post_tool_use.mcp");
        assert_eq!(tool(&event).1, ToolKind::Other);
        assert!(event.result.expect("result").success);
        let event = parse("post_tool_use.notebook_edit");
        assert_eq!(tool(&event).1, ToolKind::Edit);
        let event = parse("post_tool_use.edit");
        assert!(event.edit.is_some());
        let event = parse("post_tool_use.read");
        assert_eq!(tool(&event).1, ToolKind::Read);
    }

    #[test]
    fn a_failure_without_an_exit_code_line_has_none() {
        let payload = br#"{"session_id":"s","cwd":"/r","hook_event_name":"PostToolUseFailure","tool_name":"Bash",
            "tool_input":{"command":"x"},"error":"could not start the shell"}"#;
        let event = ClaudeCode.parse(None, payload).expect("parse");
        assert_eq!(
            event.result,
            Some(ToolResult {
                success: false,
                exit_code: None
            })
        );
    }

    #[test]
    fn session_and_prompt_events() {
        let event = parse("session_start");
        assert_eq!(event.phase, Phase::SessionStart);
        assert_eq!(event.raw["source"], "startup");
        assert!(event.tool.is_none());
        for name in [
            "session_start.resume",
            "session_start.compact",
            "session_start.clear",
        ] {
            assert_eq!(parse(name).phase, Phase::SessionStart);
        }
        let event = parse("user_prompt_submit");
        assert_eq!(event.phase, Phase::PromptSubmit);
        assert!(
            event
                .prompt
                .expect("prompt")
                .starts_with("Do exactly these steps")
        );
        let event = parse("session_end");
        assert_eq!(event.phase, Phase::SessionEnd);
        assert_eq!(event.raw["reason"], "other");
    }

    #[test]
    fn bad_payloads_recover_what_they_can() {
        let adapter = ClaudeCode;
        let err = adapter.parse(None, b"not json").unwrap_err();
        assert_eq!((err.harness_session_id, err.cwd), (None, None));

        let err = adapter.parse(None, b"[1,2]").unwrap_err();
        assert!(err.message.contains("not a JSON object"));

        let err = adapter
            .parse(
                None,
                br#"{"session_id":"s1","cwd":"/w","hook_event_name":"Stop"}"#,
            )
            .unwrap_err();
        assert!(err.message.contains("unsupported hook event"), "{err}");
        assert_eq!(err.harness_session_id.as_deref(), Some("s1"));
        assert_eq!(err.cwd.as_deref(), Some("/w"));

        // A write whose target field vanished is an unknown payload, not an ungated write.
        let err = adapter
            .parse(
                None,
                br#"{"session_id":"s2","cwd":"/w","hook_event_name":"PreToolUse","tool_name":"Write","tool_input":{"path":"/w/a"}}"#,
            )
            .unwrap_err();
        assert!(err.message.contains("no file_path"), "{err}");
        assert_eq!(err.harness_session_id.as_deref(), Some("s2"));

        for payload in [
            &br#"{"cwd":"/w","hook_event_name":"SessionStart"}"#[..],
            br#"{"session_id":"s","hook_event_name":"SessionStart"}"#,
            br#"{"session_id":"s","cwd":"/w"}"#,
            br#"{"session_id":"s","cwd":"/w","hook_event_name":"PreToolUse"}"#,
            br#"{"session_id":"s","cwd":"/w","hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{}}"#,
        ] {
            assert!(adapter.parse(None, payload).is_err(), "{payload:?}");
        }
    }

    #[test]
    fn the_phase_argument_is_only_a_fallback() {
        let payload = br#"{"session_id":"s","cwd":"/w"}"#;
        let event = ClaudeCode
            .parse(Some(Phase::SessionStart), payload)
            .expect("fallback");
        assert_eq!(event.phase, Phase::SessionStart);
        let event = ClaudeCode
            .parse(Some(Phase::PostTool), &fixture("session_start"))
            .expect("parse");
        assert_eq!(event.phase, Phase::SessionStart, "the payload wins");
    }

    // ----- rendering ---------------------------------------------------------------------

    fn render(name: &str, decision: &Decision) -> HookOutput {
        let event = parse(name);
        ClaudeCode.render(Some(&event), decision)
    }

    fn json(bytes: &[u8]) -> Value {
        serde_json::from_slice(bytes).expect("stdout is one JSON document")
    }

    fn fixture_json(name: &str) -> Value {
        json(&fixture(&format!("output/{name}")))
    }

    fn allow() -> Decision {
        Decision::new(Outcome::Allow, ReasonCode::Ok, "")
    }

    const REASON: &str = "src/payments/refund.ts is claimed by sara@example.com (Claude Code, active 4 min). Work on other files, or ask your pilot to override.";

    #[test]
    fn a_plain_allow_is_silent() {
        for name in [
            "pre_tool_use.edit",
            "post_tool_use.bash",
            "session_start",
            "user_prompt_submit",
            "session_end",
        ] {
            let out = render(name, &allow());
            assert_eq!(out, HookOutput::default(), "{name}");
        }
        assert_eq!(ClaudeCode.render(None, &allow()), HookOutput::default());
    }

    #[test]
    fn allow_never_says_allow() {
        let mut decision = allow();
        decision
            .warnings
            .push("the remote claims view is stale".to_owned());
        let out = render("pre_tool_use.write", &decision);
        assert_eq!(out.exit_code, 0);
        assert!(out.stderr.is_empty());
        let value = json(&out.stdout);
        assert!(!String::from_utf8_lossy(&out.stdout).contains("permissionDecision"));
        assert_eq!(value["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        assert_eq!(
            value["hookSpecificOutput"]["additionalContext"],
            "the remote claims view is stale"
        );
    }

    #[test]
    fn allow_with_context_matches_the_verified_shape() {
        let mut decision = allow();
        decision.context.push(
            "ACS lore for src/payments/: refunds must be idempotent (see lore_refund_idempotency)."
                .to_owned(),
        );
        let out = render("pre_tool_use.write", &decision);
        assert_eq!(
            json(&out.stdout),
            fixture_json("pre_tool_use.allow_with_context")
        );
    }

    #[test]
    fn deny_is_json_stderr_and_exit_two() {
        let decision = Decision::new(Outcome::Deny, ReasonCode::ClaimConflict, REASON);
        let out = render("pre_tool_use.edit", &decision);
        assert_eq!(out.exit_code, 2);
        assert_eq!(String::from_utf8_lossy(&out.stderr), REASON);
        assert_eq!(json(&out.stdout), fixture_json("pre_tool_use.deny"));
    }

    #[test]
    fn deny_appends_warnings_and_falls_back_when_the_message_is_empty() {
        let mut decision = Decision::new(Outcome::Deny, ReasonCode::SealedPath, REASON);
        decision
            .warnings
            .push("policy file invalid, using the last valid one".to_owned());
        let out = render("pre_tool_use.edit", &decision);
        let value = json(&out.stdout);
        let shown = value["hookSpecificOutput"]["permissionDecisionReason"]
            .as_str()
            .expect("reason");
        assert_eq!(
            shown,
            format!("{REASON}\npolicy file invalid, using the last valid one")
        );
        assert_eq!(String::from_utf8_lossy(&out.stderr), shown);

        let decision = Decision::new(Outcome::Deny, ReasonCode::SealedPath, "");
        let out = render("pre_tool_use.edit", &decision);
        let value = json(&out.stdout);
        let shown = value["hookSpecificOutput"]["permissionDecisionReason"]
            .as_str()
            .expect("reason");
        assert!(shown.contains("sealed_path"), "{shown}");
    }

    #[test]
    fn queue_is_a_deny() {
        let decision = Decision::new(Outcome::Queue, ReasonCode::ClaimConflict, REASON);
        let out = render("pre_tool_use.edit", &decision);
        assert_eq!(out.exit_code, 2);
        assert_eq!(
            json(&out.stdout)["hookSpecificOutput"]["permissionDecision"],
            "deny"
        );
    }

    #[test]
    fn ask_prompts_with_the_reason_and_exits_zero() {
        let decision = Decision::new(
            Outcome::Ask,
            ReasonCode::DestructiveBudget,
            "Deleting 63 files in one command. Confirm with your pilot.",
        );
        let out = render("pre_tool_use.bash.rm", &decision);
        assert_eq!(out.exit_code, 0);
        assert!(out.stderr.is_empty());
        assert_eq!(json(&out.stdout), fixture_json("pre_tool_use.ask"));
    }

    #[test]
    fn context_events_use_their_own_event_name() {
        let mut decision = allow();
        decision.context.push(
            "ACS: 2 live claims in this repository (src/payments/** held by sara@example.com). Call acs_intent first."
                .to_owned(),
        );
        let out = render("session_start", &decision);
        assert_eq!(json(&out.stdout), fixture_json("session_start.context"));
        assert_eq!(out.exit_code, 0);

        let mut decision = allow();
        decision
            .context
            .push("ACS notice: sara@example.com released src/payments/refund.ts.".to_owned());
        let out = render("user_prompt_submit", &decision);
        assert_eq!(
            json(&out.stdout),
            fixture_json("user_prompt_submit.context")
        );

        let mut decision = allow();
        decision.context.push(
            "ACS: this edit was attributed to your session and added to your claim on src/payments/refund.ts."
                .to_owned(),
        );
        let out = render("post_tool_use.edit", &decision);
        assert_eq!(json(&out.stdout), fixture_json("post_tool_use.context"));

        let out = render("post_tool_use_failure.bash", &decision);
        assert_eq!(
            json(&out.stdout)["hookSpecificOutput"]["hookEventName"],
            "PostToolUseFailure"
        );
    }

    #[test]
    fn message_context_and_warnings_all_reach_context_events() {
        let mut decision = Decision::new(Outcome::Allow, ReasonCode::Ok, "m");
        decision.context = vec!["c1".to_owned(), "c2".to_owned()];
        decision.warnings = vec!["w".to_owned()];
        let out = render("session_start", &decision);
        assert_eq!(
            json(&out.stdout)["hookSpecificOutput"]["additionalContext"],
            "m\nc1\nc2\nw"
        );
    }

    #[test]
    fn session_end_output_is_always_empty() {
        let mut decision = allow();
        decision.warnings.push("x".to_owned());
        assert_eq!(render("session_end", &decision), HookOutput::default());
    }

    #[test]
    fn long_text_is_capped_below_claude_codes_limit() {
        let mut decision = allow();
        decision.context.push("é".repeat(CONTEXT_LIMIT * 2));
        let out = render("session_start", &decision);
        let value = json(&out.stdout);
        let text = value["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .expect("text");
        assert!(
            text.chars().count() <= CONTEXT_LIMIT,
            "{}",
            text.chars().count()
        );
        assert!(text.ends_with("[ACS: text shortened]"));

        let decision = Decision::new(
            Outcome::Deny,
            ReasonCode::ClaimConflict,
            "x".repeat(CONTEXT_LIMIT + 5),
        );
        let out = render("pre_tool_use.edit", &decision);
        assert!(out.stderr.len() <= CONTEXT_LIMIT + 4);
    }

    #[test]
    fn a_decision_without_an_event_blocks_through_the_exit_code() {
        let decision = Decision::new(Outcome::Deny, ReasonCode::SealedPath, REASON);
        let out = ClaudeCode.render(None, &decision);
        assert_eq!(out.exit_code, 2);
        assert!(out.stdout.is_empty());
        assert_eq!(String::from_utf8_lossy(&out.stderr), REASON);
    }

    #[test]
    fn the_plugin_registers_exactly_the_events_the_adapter_parses() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../integrations/claude-code");
        let hooks: Value = serde_json::from_slice(
            &std::fs::read(dir.join("hooks/hooks.json")).expect("hooks.json"),
        )
        .expect("valid JSON");
        let hooks = hooks["hooks"].as_object().expect("hooks object");
        let mut events: Vec<&str> = hooks.keys().map(String::as_str).collect();
        events.sort_unstable();
        assert_eq!(
            events,
            [
                "PostToolUse",
                "PostToolUseFailure",
                "PreToolUse",
                "SessionEnd",
                "SessionStart",
                "UserPromptSubmit"
            ]
        );
        for (event, groups) in hooks {
            let group = &groups[0];
            assert!(group.get("matcher").is_none(), "{event}: no matcher");
            let handler = &group["hooks"][0];
            assert_eq!(handler["type"], "command");
            assert_eq!(handler["command"], "acs", "{event}");
            assert_eq!(handler["args"], json!(["hook", "claude-code"]), "{event}");
            assert!(
                handler.get("onFailure").is_none(),
                "{event}: fail open by default"
            );
        }
        let mcp: Value =
            serde_json::from_slice(&std::fs::read(dir.join(".mcp.json")).expect(".mcp.json"))
                .expect("valid JSON");
        assert_eq!(
            mcp["mcpServers"]["acs"]["args"],
            json!(["mcp", "--harness", "claude-code"])
        );
        let manifest: Value = serde_json::from_slice(
            &std::fs::read(dir.join(".claude-plugin/plugin.json")).expect("plugin.json"),
        )
        .expect("valid JSON");
        assert_eq!(manifest["name"], "acs");
    }

    #[test]
    fn the_registry_knows_claude_code() {
        let adapter = crate::by_name("claude-code").expect("registered");
        assert_eq!(adapter.name(), "claude-code");
        assert!(crate::names().contains(&"claude-code"));
    }
}
