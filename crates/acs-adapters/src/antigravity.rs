//! The Antigravity adapter: `hooks.json` command hooks of the Antigravity CLI (`agy`) and the
//! Antigravity app, which share one agent engine.
//!
//! The formats this module relies on, their sources and every gap are recorded in
//! `docs/adapters/antigravity.md`; the decisions beyond the plan are in decision 0014. In short:
//!
//! - Antigravity writes one camelCase JSON object to the hook's stdin and does **not** name the
//!   event, so the hook command line carries `--phase` (`acs hook antigravity --phase pre_tool`).
//!   Without `--phase` the phase is inferred from the payload's fields, which is a fallback only.
//! - `PreToolUse` fails **closed** in Antigravity (a crash, an empty object or an unknown decision
//!   blocks the tool), so [`render`](Adapter::render) always prints exactly one explicit JSON
//!   decision and exits 0.
//! - `PreToolUse` cannot carry context. Warnings ride in the `reason` of a deny or ask and are
//!   dropped on an allow. `PreInvocation` can inject context through `injectSteps`.
//! - `allow` is never emitted: the desktop app treats it as approval and would skip the user's own
//!   permission prompt. ACS "no objection" is `{"decision":"ask"}`, which defers to Antigravity's
//!   normal permission flow.

use acs_core::{
    Decision, EditInfo, Outcome, Phase, SpecVersion, Tool, ToolEvent, ToolKind, ToolResult,
};
use serde_json::{Map, Value, json};

use super::{Adapter, AdapterError, HookOutput};

/// The adapter registered as `antigravity`.
#[derive(Debug, Clone, Copy, Default)]
pub struct Antigravity;

/// How deep [`collect_path_values`] looks into nested arguments.
const MAX_ARG_DEPTH: usize = 4;

/// Tools that create or overwrite a whole file, across Antigravity builds (`write_to_file` up to
/// 1.4.2, `write_file` from 1.4.3, `create_file` in the SDK).
const WRITE_TOOLS: &[&str] = &["write_to_file", "write_file", "create_file"];
/// Tools that change part of a file.
const EDIT_TOOLS: &[&str] = &[
    "replace_file_content",
    "multi_replace_file_content",
    "edit_file",
];
/// Tools that only read files or list directories.
const READ_TOOLS: &[&str] = &[
    "view_file",
    "read_file",
    "view_code_item",
    "view_file_outline",
    "list_dir",
    "list_directory",
    "find_by_name",
    "find_file",
    "grep_search",
    "search_directory",
];
/// Known tools that neither run commands nor touch files by path.
const OTHER_TOOLS: &[&str] = &[
    "call_mcp_tool",
    "invoke_subagent",
    "define_subagent",
    "manage_subagents",
    "send_message",
    "manage_task",
    "schedule",
    "ask_question",
    "ask_permission",
    "list_permissions",
    "search_web",
    "read_url_content",
    "generate_image",
    "run_workflow",
    "list_resources",
    "read_resource",
    "finish",
];
/// Argument keys that name the file a write or edit tool changes.
const FILE_KEYS: &[&str] = &["targetfile", "path", "absolutepath", "filepath", "file"];
/// Additional keys of read tools.
const READ_KEYS: &[&str] = &["directorypath", "searchpath", "searchdirectory"];

impl Adapter for Antigravity {
    fn name(&self) -> &'static str {
        "antigravity"
    }

    fn parse(&self, phase: Option<Phase>, payload: &[u8]) -> Result<ToolEvent, AdapterError> {
        let value: Value = serde_json::from_slice(payload)
            .map_err(|err| AdapterError::new(format!("not JSON: {err}")))?;
        let Value::Object(map) = &value else {
            return Err(AdapterError::new("payload is not a JSON object"));
        };
        let recovered = |message: String| {
            let mut error = AdapterError::new(message);
            error.harness_session_id = session_id(map);
            error.cwd = workspace(map).or_else(|| text(map, "cwd"));
            error
        };

        let Some(harness_session_id) = session_id(map) else {
            return Err(recovered(
                "payload has no conversationId (or session_id)".to_owned(),
            ));
        };
        let tool_call = get(map, "toolCall")
            .or_else(|| get(map, "tool_call"))
            .and_then(Value::as_object);
        let Some(phase) = phase.or_else(|| infer_phase(map, tool_call.is_some())) else {
            return Err(recovered(
                "cannot tell which hook event this is: pass --phase".to_owned(),
            ));
        };
        let phase = refine_phase(phase, map);

        let mut event = ToolEvent {
            spec: SpecVersion,
            harness: "antigravity".to_owned(),
            harness_session_id,
            phase,
            cwd: workspace(map)
                .or_else(|| text(map, "cwd"))
                .unwrap_or_default(),
            tool: None,
            paths: Vec::new(),
            edit: None,
            command: None,
            result: None,
            prompt: None,
            raw: value.clone(),
        };

        match phase {
            Phase::PreTool | Phase::PostTool => {
                let Some(call) = tool_call else {
                    return Err(recovered("tool event without toolCall".to_owned()));
                };
                let Some(name) = text(call, "name") else {
                    return Err(recovered("toolCall has no name".to_owned()));
                };
                let empty = Map::new();
                let args = get(call, "args")
                    .and_then(Value::as_object)
                    .unwrap_or(&empty);
                apply_tool(&mut event, &name, args);
                if phase == Phase::PostTool {
                    // PostToolUse carries `error` (a string, empty on success) and, although the
                    // schema declares one, never a result or an exit code.
                    event.result =
                        get(map, "error")
                            .and_then(Value::as_str)
                            .map(|error| ToolResult {
                                success: error.is_empty(),
                                exit_code: None,
                            });
                }
            }
            Phase::SessionStart | Phase::PromptSubmit => {
                event.prompt = text(map, "lastUserInput");
            }
            Phase::SessionEnd => {}
        }
        Ok(event)
    }

    fn render(&self, event: Option<&ToolEvent>, decision: &Decision) -> HookOutput {
        // Without an event the payload was not understood: the pipeline allows, and the safe
        // output for the event that fails closed is the same as for a pre-tool allow.
        let phase = event.map_or(Phase::PreTool, |event| event.phase);
        let value = match phase {
            Phase::PreTool => render_pre_tool(decision),
            Phase::SessionStart | Phase::PromptSubmit => render_pre_invocation(decision),
            Phase::PostTool | Phase::SessionEnd => json!({}),
        };
        // A `Value` of this shape always serializes; fall back to the safe pre-tool answer
        // rather than panic or print nothing (an empty answer blocks the tool).
        let mut stdout =
            serde_json::to_vec(&value).unwrap_or_else(|_| br#"{"decision":"ask"}"#.to_vec());
        stdout.push(b'\n');
        HookOutput {
            stdout,
            stderr: Vec::new(),
            exit_code: 0,
        }
    }
}

/// The `PreToolUse` answer.
fn render_pre_tool(decision: &Decision) -> Value {
    match decision.outcome {
        // `queue` is reserved and must be treated as a deny.
        Outcome::Deny | Outcome::Queue => {
            json!({"decision": "deny", "reason": reason_text(decision)})
        }
        Outcome::Ask => json!({"decision": "force_ask", "reason": reason_text(decision)}),
        // "No objection". Never `allow`, see the module documentation; warnings have no channel.
        Outcome::Allow => json!({"decision": "ask"}),
    }
}

/// The `PreInvocation` answer: injected context, or nothing.
fn render_pre_invocation(decision: &Decision) -> Value {
    let text = decision
        .context
        .iter()
        .chain(&decision.warnings)
        .map(String::as_str)
        .filter(|part| !part.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    if text.is_empty() {
        json!({})
    } else {
        json!({"injectSteps": [{"ephemeralMessage": text}]})
    }
}

/// The message the agent sees with a deny or ask: the decision's message, then its warnings.
fn reason_text(decision: &Decision) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if !decision.message.trim().is_empty() {
        parts.push(&decision.message);
    }
    parts.extend(
        decision
            .warnings
            .iter()
            .map(String::as_str)
            .filter(|warning| !warning.trim().is_empty()),
    );
    if parts.is_empty() {
        format!("Blocked by ACS ({:?}).", decision.reason_code)
    } else {
        parts.join("\n")
    }
}

/// Case-insensitive key lookup.
fn get<'a>(map: &'a Map<String, Value>, key: &str) -> Option<&'a Value> {
    map.get(key).or_else(|| {
        map.iter()
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(key))
            .map(|(_, value)| value)
    })
}

/// A non-empty string field, looked up case-insensitively.
fn text(map: &Map<String, Value>, key: &str) -> Option<String> {
    get(map, key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

/// The harness session id: `conversationId`, or the Claude style `session_id` that one guide
/// documents for Antigravity.
fn session_id(map: &Map<String, Value>) -> Option<String> {
    text(map, "conversationId").or_else(|| text(map, "session_id"))
}

/// The first workspace root.
fn workspace(map: &Map<String, Value>) -> Option<String> {
    get(map, "workspacePaths")
        .and_then(Value::as_array)
        .and_then(|roots| roots.iter().find_map(Value::as_str))
        .filter(|root| !root.is_empty())
        .map(str::to_owned)
}

/// Guesses the phase when the command line gave none. Antigravity's payload has no event name;
/// this is a fallback for hooks installed without `--phase`.
fn infer_phase(map: &Map<String, Value>, has_tool_call: bool) -> Option<Phase> {
    if has_tool_call {
        Some(if get(map, "error").is_some() {
            Phase::PostTool
        } else {
            Phase::PreTool
        })
    } else if get(map, "invocationNum").is_some() {
        Some(Phase::PromptSubmit)
    } else {
        None
    }
}

/// Turns a `PreInvocation` payload into `session_start` when it is the conversation's first
/// invocation, `prompt_submit` otherwise. Other phases are unchanged.
///
/// Antigravity has no session-start event. The first invocation is recognised by
/// `invocationNum <= 1` together with `initialNumSteps <= 1` (the user's own input is probably
/// step 0 or 1); a missing field does not count against it, and with neither field the event is a
/// plain `prompt_submit`. Whether `invocationNum` restarts every turn is unverified: if it does,
/// every turn starts with a status injection, which is noisy but harmless.
fn refine_phase(phase: Phase, map: &Map<String, Value>) -> Phase {
    if !matches!(phase, Phase::SessionStart | Phase::PromptSubmit) {
        return phase;
    }
    let number = |key: &str| get(map, key).and_then(Value::as_i64);
    let (Some(invocation), steps) = (number("invocationNum"), number("initialNumSteps")) else {
        return Phase::PromptSubmit;
    };
    if invocation <= 1 && steps.is_none_or(|steps| steps <= 1) {
        Phase::SessionStart
    } else {
        Phase::PromptSubmit
    }
}

/// Fills the tool, paths, edit and command of `event` from a tool call.
fn apply_tool(event: &mut ToolEvent, name: &str, args: &Map<String, Value>) {
    let lower = name.to_ascii_lowercase();
    let is = |list: &[&str]| list.contains(&lower.as_str());
    let mut tool_name = name.to_owned();

    let kind = if lower == "run_command" {
        event.command = text(args, "CommandLine")
            .or_else(|| text(args, "command"))
            .or_else(|| text(args, "commandLine"));
        // `Cwd` was an argument up to 1.4.2: the directory the command runs in.
        if let Some(cwd) = text(args, "Cwd") {
            event.cwd = cwd;
        }
        // `agy` runs commands in PowerShell on Windows, but names the tool `run_command` on every
        // OS. Decision 0011 picks the dialect from a name containing `powershell`.
        if is_windows_path(&event.cwd) {
            tool_name = format!("{name} (powershell)");
        }
        ToolKind::Shell
    } else if is(WRITE_TOOLS) {
        event.paths = file_paths(args, false);
        ToolKind::Write
    } else if is(EDIT_TOOLS) {
        event.paths = file_paths(args, false);
        event.edit = edit_info(&lower, args);
        ToolKind::Edit
    } else if is(READ_TOOLS) {
        event.paths = file_paths(args, true);
        ToolKind::Read
    } else if is(OTHER_TOOLS) {
        ToolKind::Other
    } else {
        // An unknown tool: tool names change between Antigravity builds. One that names paths may
        // change them, so it is a write (conservative); one that does not is `other`.
        let mut paths = Vec::new();
        collect_path_values(&Value::Object(args.clone()), 0, &mut paths);
        if paths.is_empty() {
            ToolKind::Other
        } else {
            event.paths = paths;
            ToolKind::Write
        }
    };
    event.tool = Some(Tool {
        name: tool_name,
        kind,
    });
}

/// The file arguments of a file tool, in a stable order, without duplicates.
fn file_paths(args: &Map<String, Value>, include_read_keys: bool) -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    for key in FILE_KEYS
        .iter()
        .chain(READ_KEYS.iter().filter(|_| include_read_keys))
    {
        if let Some(value) = get(args, key).and_then(Value::as_str)
            && !value.is_empty()
        {
            let value = decode_file_uri(value);
            if !paths.contains(&value) {
                paths.push(value);
            }
        }
    }
    paths
}

/// The text change of an edit tool, when it is exactly one replacement.
fn edit_info(tool: &str, args: &Map<String, Value>) -> Option<EditInfo> {
    let pair = |map: &Map<String, Value>| {
        let old =
            text_allow_empty(map, "TargetContent").or_else(|| text_allow_empty(map, "Target"))?;
        let new = text_allow_empty(map, "ReplacementContent")
            .or_else(|| text_allow_empty(map, "Replacement"))?;
        Some(EditInfo {
            old_text: old,
            new_text: new,
        })
    };
    if tool == "multi_replace_file_content" {
        let chunks = get(args, "ReplacementChunks")?.as_array()?;
        match chunks.as_slice() {
            [Value::Object(only)] => pair(only),
            _ => None,
        }
    } else {
        pair(args)
    }
}

/// A string field that may be empty (a replacement can delete text).
fn text_allow_empty(map: &Map<String, Value>, key: &str) -> Option<String> {
    get(map, key).and_then(Value::as_str).map(str::to_owned)
}

/// Whether `key` looks like it names a path, for tools this adapter does not know.
fn is_path_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    [
        "path",
        "paths",
        "file",
        "files",
        "dir",
        "dirs",
        "directory",
        "uri",
        "uris",
    ]
    .iter()
    .any(|suffix| key.ends_with(suffix))
}

/// Collects the string values under path-like keys, at any depth up to [`MAX_ARG_DEPTH`].
fn collect_path_values(value: &Value, depth: usize, out: &mut Vec<String>) {
    if depth > MAX_ARG_DEPTH {
        return;
    }
    match value {
        Value::Object(map) => {
            for (key, inner) in map {
                if is_path_key(key) {
                    push_strings(inner, out);
                } else {
                    collect_path_values(inner, depth + 1, out);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_path_values(item, depth + 1, out);
            }
        }
        _ => {}
    }
}

/// Pushes `value` (a string, or the strings of an array) as paths.
fn push_strings(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(s) if !s.is_empty() => {
            let path = decode_file_uri(s);
            if !out.contains(&path) {
                out.push(path);
            }
        }
        Value::Array(items) => {
            for item in items {
                push_strings(item, out);
            }
        }
        _ => {}
    }
}

/// Turns a `file:` URI into a path (percent-decoded; `file:///C:/x` becomes `C:/x`). Anything
/// else is returned unchanged.
fn decode_file_uri(raw: &str) -> String {
    let Some(rest) = raw
        .get(..7)
        .filter(|scheme| scheme.eq_ignore_ascii_case("file://"))
        .map(|_| &raw[7..])
    else {
        return raw.to_owned();
    };
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let bytes = rest.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hex) = rest.get(i + 1..i + 3)
            && let Ok(byte) = u8::from_str_radix(hex, 16)
        {
            decoded.push(byte);
            i += 3;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    let decoded = String::from_utf8_lossy(&decoded).into_owned();
    // `/C:/dir` is a drive path.
    let drive = decoded.as_bytes();
    if drive.len() >= 3 && drive[0] == b'/' && drive[1].is_ascii_alphabetic() && drive[2] == b':' {
        decoded[1..].to_owned()
    } else {
        decoded
    }
}

/// Whether `path` is a Windows path: a drive letter or a UNC prefix.
fn is_windows_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    let drive = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    drive || path.starts_with("\\\\")
}

#[cfg(test)]
mod tests {
    use acs_core::ReasonCode;

    use super::*;

    fn parse(phase: Option<Phase>, json: &str) -> Result<ToolEvent, AdapterError> {
        Antigravity.parse(phase, json.as_bytes())
    }

    fn tool_event(phase: Phase, call: &str) -> ToolEvent {
        let json =
            format!(r#"{{"conversationId":"c1","workspacePaths":["/w"],"toolCall":{call}}}"#);
        parse(Some(phase), &json).unwrap()
    }

    #[test]
    fn keys_are_matched_case_insensitively() {
        let event = tool_event(
            Phase::PreTool,
            r#"{"name":"Write_File","args":{"PATH":"/w/a.rs","content":"x"}}"#,
        );
        let tool = event.tool.unwrap();
        assert_eq!(tool.kind, ToolKind::Write);
        assert_eq!(event.paths, vec!["/w/a.rs"]);
    }

    #[test]
    fn unknown_tools_with_path_arguments_are_writes() {
        let event = tool_event(
            Phase::PreTool,
            r#"{"name":"patch_workspace","args":{"Patches":[{"TargetFile":"/w/a.rs"}],"DestUri":"file:///C:/w/b%20c.txt","Note":"x"}}"#,
        );
        assert_eq!(event.tool.unwrap().kind, ToolKind::Write);
        // Argument keys iterate in sorted order, so the order is stable but not positional.
        assert_eq!(event.paths, vec!["C:/w/b c.txt", "/w/a.rs"]);

        let event = tool_event(Phase::PreTool, r#"{"name":"mystery","args":{"Q":"x"}}"#);
        assert_eq!(event.tool.unwrap().kind, ToolKind::Other);
        assert!(event.paths.is_empty());
    }

    #[test]
    fn edit_info_needs_exactly_one_replacement() {
        let single = tool_event(
            Phase::PreTool,
            r#"{"name":"multi_replace_file_content","args":{"TargetFile":"/w/a","ReplacementChunks":[{"TargetContent":"a","ReplacementContent":""}]}}"#,
        );
        assert_eq!(
            single.edit,
            Some(EditInfo {
                old_text: "a".into(),
                new_text: String::new()
            })
        );
        let two = tool_event(
            Phase::PreTool,
            r#"{"name":"multi_replace_file_content","args":{"TargetFile":"/w/a","ReplacementChunks":[{"TargetContent":"a","ReplacementContent":"b"},{"TargetContent":"c","ReplacementContent":"d"}]}}"#,
        );
        assert_eq!(two.edit, None);
        assert_eq!(two.paths, vec!["/w/a"]);
    }

    #[test]
    fn windows_workspaces_name_the_shell_tool_powershell() {
        let json = r#"{"conversationId":"c1","workspacePaths":["C:\\Users\\dev\\p"],"toolCall":{"name":"run_command","args":{"CommandLine":"dir"}}}"#;
        let event = parse(Some(Phase::PreTool), json).unwrap();
        assert_eq!(event.tool.unwrap().name, "run_command (powershell)");
        assert_eq!(event.cwd, "C:\\Users\\dev\\p");
        let posix = tool_event(
            Phase::PreTool,
            r#"{"name":"run_command","args":{"CommandLine":"ls"}}"#,
        );
        assert_eq!(posix.tool.unwrap().name, "run_command");
    }

    #[test]
    fn a_cwd_argument_sets_the_event_cwd() {
        let event = tool_event(
            Phase::PreTool,
            r#"{"name":"run_command","args":{"CommandLine":"ls","Cwd":"/w/sub"}}"#,
        );
        assert_eq!(event.cwd, "/w/sub");
        assert_eq!(event.command.as_deref(), Some("ls"));
    }

    #[test]
    fn post_tool_result_comes_from_error_only() {
        let ok = parse(
            Some(Phase::PostTool),
            r#"{"conversationId":"c","error":"","toolCall":{"name":"run_command","args":{"CommandLine":"x"},"result":"ignored"}}"#,
        )
        .unwrap();
        assert_eq!(
            ok.result,
            Some(ToolResult {
                success: true,
                exit_code: None
            })
        );
        let failed = parse(
            Some(Phase::PostTool),
            r#"{"conversationId":"c","error":"exit status 1","toolCall":{"name":"run_command","args":{}}}"#,
        )
        .unwrap();
        assert_eq!(failed.result.map(|r| r.success), Some(false));
        let none = parse(
            Some(Phase::PostTool),
            r#"{"conversationId":"c","toolCall":{"name":"run_command","args":{}}}"#,
        )
        .unwrap();
        assert_eq!(none.result, None);
    }

    #[test]
    fn first_invocation_is_session_start() {
        let phase = |json: &str| parse(Some(Phase::PromptSubmit), json).unwrap().phase;
        assert_eq!(
            phase(r#"{"conversationId":"c","invocationNum":1,"initialNumSteps":0}"#),
            Phase::SessionStart
        );
        assert_eq!(
            phase(r#"{"conversationId":"c","invocationNum":0}"#),
            Phase::SessionStart
        );
        assert_eq!(
            phase(r#"{"conversationId":"c","invocationNum":2,"initialNumSteps":5}"#),
            Phase::PromptSubmit
        );
        assert_eq!(
            phase(r#"{"conversationId":"c","invocationNum":1,"initialNumSteps":9}"#),
            Phase::PromptSubmit
        );
        assert_eq!(phase(r#"{"conversationId":"c"}"#), Phase::PromptSubmit);
        let event = parse(
            Some(Phase::PromptSubmit),
            r#"{"conversationId":"c","invocationNum":3,"lastUserInput":"fix it"}"#,
        )
        .unwrap();
        assert_eq!(event.prompt.as_deref(), Some("fix it"));
    }

    #[test]
    fn the_phase_is_inferred_when_the_command_line_gives_none() {
        let pre = parse(
            None,
            r#"{"conversationId":"c","toolCall":{"name":"view_file","args":{}}}"#,
        );
        assert_eq!(pre.unwrap().phase, Phase::PreTool);
        let post = parse(
            None,
            r#"{"conversationId":"c","error":"","toolCall":{"name":"view_file","args":{}}}"#,
        );
        assert_eq!(post.unwrap().phase, Phase::PostTool);
        let invocation = parse(None, r#"{"conversationId":"c","invocationNum":4}"#);
        assert_eq!(invocation.unwrap().phase, Phase::PromptSubmit);
        let err = parse(None, r#"{"conversationId":"c","workspacePaths":["/w"]}"#).unwrap_err();
        assert_eq!(err.harness_session_id.as_deref(), Some("c"));
        assert_eq!(err.cwd.as_deref(), Some("/w"));
    }

    #[test]
    fn garbled_payloads_are_errors_that_recover_what_they_can() {
        assert!(parse(Some(Phase::PreTool), "nope").is_err());
        assert!(parse(Some(Phase::PreTool), "[1]").is_err());
        let err = parse(
            Some(Phase::PreTool),
            r#"{"conversationId":"c9","workspacePaths":["/w"],"toolCall":{"args":{}}}"#,
        )
        .unwrap_err();
        assert_eq!(err.harness_session_id.as_deref(), Some("c9"));
        assert_eq!(err.cwd.as_deref(), Some("/w"));
        let err = parse(
            Some(Phase::PreTool),
            r#"{"conversationId":"c9","workspacePaths":["/w"]}"#,
        )
        .unwrap_err();
        assert!(err.message.contains("toolCall"));
        let err = parse(
            Some(Phase::PreTool),
            r#"{"cwd":"/x","toolCall":{"name":"a"}}"#,
        )
        .unwrap_err();
        assert_eq!(err.cwd.as_deref(), Some("/x"));
        // The Claude style session id is accepted too.
        let ok = parse(
            Some(Phase::PreTool),
            r#"{"session_id":"s1","cwd":"/x","toolCall":{"name":"view_file","args":{}}}"#,
        )
        .unwrap();
        assert_eq!(ok.harness_session_id, "s1");
        assert_eq!(ok.cwd, "/x");
    }

    fn decision(outcome: Outcome, message: &str) -> Decision {
        Decision::new(outcome, ReasonCode::Ok, message)
    }

    fn render(phase: Phase, decision: &Decision) -> Value {
        let event = tool_event(Phase::PreTool, r#"{"name":"view_file","args":{}}"#);
        let event = ToolEvent { phase, ..event };
        let out = Antigravity.render(Some(&event), decision);
        assert_eq!(out.exit_code, 0);
        assert!(out.stderr.is_empty());
        assert!(out.stdout.ends_with(b"\n"));
        serde_json::from_slice(&out.stdout).unwrap()
    }

    #[test]
    fn pre_tool_outcomes() {
        let mut deny = decision(Outcome::Deny, "claimed by sara");
        deny.warnings.push("remote view is stale".into());
        assert_eq!(
            render(Phase::PreTool, &deny),
            json!({"decision":"deny","reason":"claimed by sara\nremote view is stale"})
        );
        assert_eq!(
            render(Phase::PreTool, &decision(Outcome::Queue, "later"))["decision"],
            "deny"
        );
        assert_eq!(
            render(Phase::PreTool, &decision(Outcome::Ask, "outside the repo")),
            json!({"decision":"force_ask","reason":"outside the repo"})
        );
        // Never `allow`; warnings and context have no channel on an allow.
        let mut allow = decision(Outcome::Allow, "");
        allow.warnings.push("w".into());
        allow.context.push("c".into());
        assert_eq!(render(Phase::PreTool, &allow), json!({"decision":"ask"}));
        // An empty deny message still tells the agent something.
        let reason = render(Phase::PreTool, &decision(Outcome::Deny, ""))["reason"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(reason.starts_with("Blocked by ACS"));
    }

    #[test]
    fn unparsed_payloads_render_the_safe_pre_tool_answer() {
        let out = Antigravity.render(None, &decision(Outcome::Allow, ""));
        assert_eq!(out.stdout, b"{\"decision\":\"ask\"}\n");
        assert_eq!(out.exit_code, 0);
    }

    #[test]
    fn invocation_output_injects_only_when_there_is_something_to_say() {
        for phase in [Phase::SessionStart, Phase::PromptSubmit] {
            assert_eq!(render(phase, &decision(Outcome::Allow, "")), json!({}));
            let mut with = decision(Outcome::Allow, "");
            with.context.push("status".into());
            with.warnings.push("careful".into());
            assert_eq!(
                render(phase, &with),
                json!({"injectSteps":[{"ephemeralMessage":"status\n\ncareful"}]})
            );
        }
    }

    #[test]
    fn other_phases_print_an_empty_object() {
        for phase in [Phase::PostTool, Phase::SessionEnd] {
            assert_eq!(render(phase, &decision(Outcome::Allow, "")), json!({}));
        }
    }

    #[test]
    fn file_uris_are_decoded() {
        assert_eq!(decode_file_uri("file:///home/a%20b/c"), "/home/a b/c");
        assert_eq!(decode_file_uri("FILE:///C:/x/y"), "C:/x/y");
        assert_eq!(decode_file_uri("/plain/path"), "/plain/path");
        assert_eq!(decode_file_uri("fil"), "fil");
    }
}
