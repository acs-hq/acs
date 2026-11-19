//! The Codex adapter: command hooks of the OpenAI Codex CLI (`codex`, 0.170 and later).
//!
//! Payload and output shapes, with sources and the real captures they were checked against, are
//! in `docs/adapters/codex.md`; fixtures are in `tests/fixtures/codex/`; decision 0047 records the
//! choices. In short:
//!
//! - **Input** (stdin): one JSON object, Claude Code's shape with Codex extensions (`turn_id`,
//!   `model`). `hook_event_name` selects the phase (`SessionStart`, `UserPromptSubmit`,
//!   `PreToolUse`, `PostToolUse`, `SessionEnd`); other events are an [`AdapterError`].
//! - **Tools**: `Bash` runs a shell command (`tool_input.command`). Codex runs it with PowerShell
//!   on Windows, so a `Bash` call whose `cwd` is a Windows path becomes a `PowerShell` tool here
//!   (the original name stays in `raw.tool_name`). `apply_patch` carries the whole patch in
//!   `tool_input.command`; the files it touches are read from its `*** Add File:`,
//!   `*** Update File:`, `*** Delete File:` and `*** Move to:` lines. `mcp__<server>__<tool>` and
//!   the rest are `other`.
//! - **Output**: allow is an empty stdout or an `additionalContext` document; deny is the JSON
//!   deny document on stdout, the same reason on stderr and exit code 2. Codex does not support
//!   `permissionDecision: "ask"` (the hook run fails and the tool call goes ahead), so `ask` and
//!   `queue` are rendered as deny, with a line saying a person can let the call through.

use acs_core::{
    Decision, EditInfo, Outcome, Phase, SpecVersion, Tool, ToolEvent, ToolKind, ToolResult,
};
use serde_json::{Map, Value, json};

use super::{Adapter, AdapterError, HookOutput};

/// Codex caps `additionalContext` at 2,500 tokens by default and spills the rest to a file.
/// Stay well below it.
const CONTEXT_LIMIT: usize = 8_000;

/// Marker appended when text is cut to [`CONTEXT_LIMIT`].
const TRUNCATED: &str = "\n[ACS: text shortened]";

/// Added to the reason when ACS wanted a person to decide: Codex hooks cannot ask.
const ASK_NOTE: &str = "Codex cannot ask your pilot from a hook, so ACS blocked this call \
                        instead. Tell your pilot what you wanted to do; they can let it \
                        through with `acs override`.";

/// The adapter registered as `codex`.
#[derive(Debug, Clone, Copy, Default)]
pub struct Codex;

impl Adapter for Codex {
    fn name(&self) -> &'static str {
        "codex"
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
        let phase = match text(map, "hook_event_name") {
            Some("SessionStart") => Phase::SessionStart,
            Some("UserPromptSubmit") => Phase::PromptSubmit,
            Some("PreToolUse") => Phase::PreTool,
            Some("PostToolUse") => Phase::PostTool,
            Some("SessionEnd") => Phase::SessionEnd,
            Some(other) => return Err(fail(format!("unsupported hook event {other:?}"))),
            None => phase.ok_or_else(|| fail("payload has no hook_event_name".to_owned()))?,
        };

        let mut event = ToolEvent {
            spec: SpecVersion,
            harness: "codex".to_owned(),
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
                let shape = classify(name, input, cwd).map_err(fail)?;
                event.tool = Some(Tool {
                    name: shape.name,
                    kind: shape.kind,
                });
                event.paths = shape.paths;
                event.edit = shape.edit;
                event.command = shape.command;
                if phase == Phase::PostTool {
                    event.result = Some(post_result(map.get("tool_response")));
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
            return match decision.outcome {
                Outcome::Allow => HookOutput::default(),
                Outcome::Deny | Outcome::Ask | Outcome::Queue => HookOutput {
                    stdout: Vec::new(),
                    stderr: reason(decision).into_bytes(),
                    exit_code: 2,
                },
            };
        };
        match event.phase {
            Phase::PreTool => match decision.outcome {
                Outcome::Allow => context_output("PreToolUse", &everything(decision)),
                Outcome::Deny | Outcome::Ask | Outcome::Queue => deny(decision),
            },
            Phase::PostTool => context_output("PostToolUse", &everything(decision)),
            Phase::SessionStart => context_output("SessionStart", &everything(decision)),
            Phase::PromptSubmit => context_output("UserPromptSubmit", &everything(decision)),
            // SessionEnd output cannot steer Codex.
            Phase::SessionEnd => HookOutput::default(),
        }
    }
}

/// How a tool call maps onto the canonical event.
struct Shape {
    name: String,
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

/// Whether `path` is a Windows path (`C:\...`, `C:/...` or `\\server\...`).
fn is_windows_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    (bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/'))
        || path.starts_with("\\\\")
}

/// Maps a Codex tool name and input to the canonical tool.
///
/// Returns a message when a tool that changes files lacks the field naming what it does: the
/// payload shape changed, and the pipeline should allow with `unknown_payload` and say so rather
/// than gate on nothing.
fn classify(name: &str, input: &Map<String, Value>, cwd: &str) -> Result<Shape, String> {
    let command = || {
        text(input, "command")
            .map(str::to_owned)
            .ok_or_else(|| format!("{name} input has no command"))
    };
    let other = || Shape {
        name: name.to_owned(),
        kind: ToolKind::Other,
        paths: Vec::new(),
        edit: None,
        command: None,
    };
    Ok(match name {
        "Bash" | "exec_command" | "shell" | "local_shell" | "unified_exec" => Shape {
            // Codex runs its shell tool with PowerShell on Windows.
            name: if is_windows_path(cwd) {
                "PowerShell".to_owned()
            } else {
                name.to_owned()
            },
            kind: ToolKind::Shell,
            command: Some(command()?),
            ..other()
        },
        "apply_patch" => {
            let patch = command()?;
            let files = patch_files(&patch);
            if files.is_empty() {
                return Err("apply_patch input names no file".to_owned());
            }
            Shape {
                kind: patch_kind(&files),
                paths: patch_paths(&files),
                ..other()
            }
        }
        // `mcp__<server>__<tool>`, `update_plan`, `spawn_agent`, `write_stdin` and every tool this
        // version does not know. None of them changes files by path; a subagent's own calls
        // arrive as separate events.
        _ => other(),
    })
}

/// One file section of a patch.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PatchFile {
    Add(String),
    Update(String),
    Delete(String),
    Move { from: String, to: String },
}

/// The file sections of an `apply_patch` patch, in order.
fn patch_files(patch: &str) -> Vec<PatchFile> {
    let mut files: Vec<PatchFile> = Vec::new();
    for line in patch.lines() {
        let line = line.trim_end_matches('\r');
        let named = |prefix: &str| {
            line.strip_prefix(prefix)
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(str::to_owned)
        };
        if let Some(path) = named("*** Add File:") {
            files.push(PatchFile::Add(path));
        } else if let Some(path) = named("*** Update File:") {
            files.push(PatchFile::Update(path));
        } else if let Some(path) = named("*** Delete File:") {
            files.push(PatchFile::Delete(path));
        } else if let Some(to) = named("*** Move to:") {
            // A move belongs to the update section just above it.
            if let Some(PatchFile::Update(from)) = files.last().cloned() {
                files.pop();
                files.push(PatchFile::Move { from, to });
            }
        }
    }
    files
}

/// The canonical kind of a patch. One kind covers one event, so a patch that deletes or moves
/// alongside other changes is a delete of every path it names: deletes are checked more
/// strictly than writes, so nothing is let through that a separate call would not be.
fn patch_kind(files: &[PatchFile]) -> ToolKind {
    let deletes = files
        .iter()
        .filter(|f| matches!(f, PatchFile::Delete(_)))
        .count();
    let moves = files
        .iter()
        .filter(|f| matches!(f, PatchFile::Move { .. }))
        .count();
    let updates = files
        .iter()
        .filter(|f| matches!(f, PatchFile::Update(_)))
        .count();
    if deletes == 0 && moves == 0 {
        if updates > 0 {
            ToolKind::Edit
        } else {
            ToolKind::Write
        }
    } else if moves == 1 && files.len() == 1 {
        ToolKind::Move
    } else {
        ToolKind::Delete
    }
}

/// Every path a patch names, sources before destinations, without repeats.
fn patch_paths(files: &[PatchFile]) -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    let mut push = |path: &str| {
        if !paths.iter().any(|p| p == path) {
            paths.push(path.to_owned());
        }
    };
    for file in files {
        match file {
            PatchFile::Add(p) | PatchFile::Update(p) | PatchFile::Delete(p) => push(p),
            PatchFile::Move { from, to } => {
                push(from);
                push(to);
            }
        }
    }
    paths
}

/// The result of a finished tool call. `apply_patch` reports `Exit code: N` on its first line;
/// a shell call reports only its output, so its exit code is unknown and it counts as done.
fn post_result(response: Option<&Value>) -> ToolResult {
    let exit_code = response
        .and_then(Value::as_str)
        .and_then(|r| r.lines().next())
        .and_then(|line| line.strip_prefix("Exit code:"))
        .and_then(|code| code.trim().parse::<i32>().ok());
    ToolResult {
        success: exit_code.is_none_or(|code| code == 0),
        exit_code,
    }
}

/// The small fields of the payload worth keeping in `raw`: identifiers and modes, never tool
/// input, tool output, prompts or transcript paths.
fn raw_summary(map: &Map<String, Value>) -> Value {
    const KEEP: &[&str] = &[
        "hook_event_name",
        "tool_name",
        "tool_use_id",
        "turn_id",
        "permission_mode",
        "source",
        "reason",
        "model",
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

/// The reason shown to the model for a blocked call.
fn reason(decision: &Decision) -> String {
    let mut text = everything(decision);
    if decision.message.is_empty() {
        let code = serde_json::to_value(decision.reason_code)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_else(|| "unknown".to_owned());
        let fallback = format!("ACS blocked this tool call ({code}). Call acs_status for details.");
        text = if text.is_empty() {
            fallback
        } else {
            format!("{fallback}\n{text}")
        };
    }
    if matches!(decision.outcome, Outcome::Ask) {
        text = format!("{text}\n{ASK_NOTE}");
    }
    cap(text)
}

fn deny(decision: &Decision) -> HookOutput {
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

#[cfg(test)]
mod tests {
    use acs_core::ReasonCode;

    use super::*;

    fn event(payload: &str) -> ToolEvent {
        Codex.parse(None, payload.as_bytes()).expect("parse")
    }

    fn pre(tool: &str, input: &str, cwd: &str) -> String {
        format!(
            r#"{{"session_id":"s1","turn_id":"t1","cwd":{cwd:?},"hook_event_name":"PreToolUse",
            "model":"m","permission_mode":"default","tool_name":"{tool}","tool_input":{input},
            "tool_use_id":"exec-1"}}"#
        )
    }

    #[test]
    fn bash_is_powershell_on_windows_and_bash_elsewhere() {
        let windows = event(&pre("Bash", r#"{"command":"git status"}"#, r"C:\work\repo"));
        let tool = windows.tool.expect("tool");
        assert_eq!(
            (tool.name.as_str(), tool.kind),
            ("PowerShell", ToolKind::Shell)
        );
        assert_eq!(windows.command.as_deref(), Some("git status"));
        assert_eq!(windows.raw["tool_name"], "Bash");

        let unix = event(&pre("Bash", r#"{"command":"ls"}"#, "/home/dev/repo"));
        assert_eq!(unix.tool.expect("tool").name, "Bash");
    }

    #[test]
    fn a_shell_call_without_a_command_is_an_unknown_payload() {
        let err = Codex
            .parse(None, pre("Bash", "{}", "/r").as_bytes())
            .unwrap_err();
        assert!(err.message.contains("no command"), "{err}");
        assert_eq!(err.harness_session_id.as_deref(), Some("s1"));
    }

    fn patch_event(patch: &str) -> ToolEvent {
        let input = json!({ "command": patch }).to_string();
        event(&pre("apply_patch", &input, "/home/dev/repo"))
    }

    fn kind_and_paths(event: &ToolEvent) -> (ToolKind, Vec<&str>) {
        (
            event.tool.as_ref().expect("tool").kind,
            event.paths.iter().map(String::as_str).collect(),
        )
    }

    #[test]
    fn patches_name_their_files_and_kind() {
        let update =
            "*** Begin Patch\n*** Update File: a.txt\n@@\n-hello\n+hello world\n*** End Patch";
        assert_eq!(
            kind_and_paths(&patch_event(update)),
            (ToolKind::Edit, vec!["a.txt"])
        );
        let add = "*** Begin Patch\n*** Add File: c.txt\n+new\n*** End Patch";
        assert_eq!(
            kind_and_paths(&patch_event(add)),
            (ToolKind::Write, vec!["c.txt"])
        );
        let delete = "*** Begin Patch\n*** Delete File: b.txt\n*** End Patch";
        assert_eq!(
            kind_and_paths(&patch_event(delete)),
            (ToolKind::Delete, vec!["b.txt"])
        );
        let moved = "*** Begin Patch\n*** Update File: d.txt\n*** Move to: e.txt\n@@\n-new\n+moved\n*** End Patch";
        assert_eq!(
            kind_and_paths(&patch_event(moved)),
            (ToolKind::Move, vec!["d.txt", "e.txt"])
        );
        let two_edits = "*** Begin Patch\r\n*** Update File: a.txt\r\n@@\r\n-x\r\n+y\r\n*** Add File: src/n.rs\r\n+fn n() {}\r\n*** End Patch";
        assert_eq!(
            kind_and_paths(&patch_event(two_edits)),
            (ToolKind::Edit, vec!["a.txt", "src/n.rs"])
        );
    }

    #[test]
    fn a_patch_that_deletes_among_other_changes_is_a_delete_of_every_path() {
        let mixed = "*** Begin Patch\n*** Update File: a.txt\n@@\n-x\n+y\n*** Delete File: b.txt\n*** End Patch";
        assert_eq!(
            kind_and_paths(&patch_event(mixed)),
            (ToolKind::Delete, vec!["a.txt", "b.txt"])
        );
        let move_and_add = "*** Begin Patch\n*** Update File: a\n*** Move to: b\n*** Add File: c\n+c\n*** End Patch";
        assert_eq!(
            kind_and_paths(&patch_event(move_and_add)),
            (ToolKind::Delete, vec!["a", "b", "c"])
        );
    }

    #[test]
    fn a_patch_without_files_is_an_unknown_payload() {
        let input = json!({ "command": "*** Begin Patch\n*** End Patch" }).to_string();
        assert!(
            Codex
                .parse(None, pre("apply_patch", &input, "/r").as_bytes())
                .is_err()
        );
    }

    #[test]
    fn mcp_and_other_tools_are_other() {
        let mcp = event(&pre("mcp__acs__acs_status", "{}", "/r"));
        assert_eq!(mcp.tool.expect("tool").kind, ToolKind::Other);
        let plan = event(&pre("update_plan", r#"{"plan":[]}"#, "/r"));
        assert_eq!(plan.tool.expect("tool").kind, ToolKind::Other);
    }

    #[test]
    fn post_tool_reads_the_patch_exit_code() {
        let ok = r#"{"session_id":"s","cwd":"/r","hook_event_name":"PostToolUse","tool_name":"apply_patch",
            "tool_input":{"command":"*** Begin Patch\n*** Add File: a\n+a\n*** End Patch"},
            "tool_response":"Exit code: 0\nWall time: 1 seconds\nOutput:\nA a\n"}"#;
        let result = event(ok).result.expect("result");
        assert_eq!((result.success, result.exit_code), (true, Some(0)));
        let failed = ok.replace("Exit code: 0", "Exit code: 1");
        let result = event(&failed).result.expect("result");
        assert_eq!((result.success, result.exit_code), (false, Some(1)));
        let shell = r#"{"session_id":"s","cwd":"/r","hook_event_name":"PostToolUse","tool_name":"Bash",
            "tool_input":{"command":"ls"},"tool_response":"a\nb\n"}"#;
        let result = event(shell).result.expect("result");
        assert_eq!((result.success, result.exit_code), (true, None));
    }

    #[test]
    fn other_events_are_unsupported_and_raw_stays_small() {
        let stop =
            r#"{"session_id":"s","cwd":"/r","hook_event_name":"Stop","stop_hook_active":false}"#;
        assert!(Codex.parse(None, stop.as_bytes()).is_err());
        let start = r#"{"session_id":"s","cwd":"/r","hook_event_name":"SessionStart","source":"startup",
            "transcript_path":"/x/rollout.jsonl","model":"m","permission_mode":"default"}"#;
        let event = event(start);
        assert_eq!(event.phase, Phase::SessionStart);
        assert!(!event.raw.to_string().contains("rollout"));
    }

    fn decision(outcome: Outcome, message: &str) -> Decision {
        Decision::new(outcome, ReasonCode::Ok, message)
    }

    #[test]
    fn deny_and_ask_block_with_the_reason_on_both_streams() {
        let event = event(&pre("Bash", r#"{"command":"rm x"}"#, "/r"));
        for outcome in [Outcome::Deny, Outcome::Ask, Outcome::Queue] {
            let out = Codex.render(Some(&event), &decision(outcome, "x is claimed by bob"));
            assert_eq!(out.exit_code, 2);
            let doc: Value = serde_json::from_slice(&out.stdout).expect("json");
            assert_eq!(doc["hookSpecificOutput"]["permissionDecision"], "deny");
            let reason = doc["hookSpecificOutput"]["permissionDecisionReason"]
                .as_str()
                .expect("reason");
            assert!(reason.starts_with("x is claimed by bob"));
            assert_eq!(String::from_utf8_lossy(&out.stderr), reason);
            assert_eq!(reason.contains("acs override"), outcome == Outcome::Ask);
        }
    }

    #[test]
    fn allow_is_silent_or_context_and_never_an_explicit_allow() {
        let event = event(&pre("Bash", r#"{"command":"ls"}"#, "/r"));
        let out = Codex.render(Some(&event), &decision(Outcome::Allow, ""));
        assert_eq!(out, HookOutput::default());
        let mut with_context = decision(Outcome::Allow, "");
        with_context
            .context
            .push("lore: read the README".to_owned());
        let out = Codex.render(Some(&event), &with_context);
        let doc: Value = serde_json::from_slice(&out.stdout).expect("json");
        assert_eq!(doc["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        assert_eq!(
            doc["hookSpecificOutput"]["additionalContext"],
            "lore: read the README"
        );
        assert!(
            doc["hookSpecificOutput"]
                .get("permissionDecision")
                .is_none()
        );
    }

    #[test]
    fn long_context_is_capped() {
        let event = event(&pre("Bash", r#"{"command":"ls"}"#, "/r"));
        let long = decision(Outcome::Allow, &"x".repeat(CONTEXT_LIMIT * 2));
        let out = Codex.render(Some(&event), &long);
        let doc: Value = serde_json::from_slice(&out.stdout).expect("json");
        let text = doc["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .expect("text");
        assert_eq!(text.chars().count(), CONTEXT_LIMIT);
        assert!(text.ends_with(TRUNCATED));
    }
}
