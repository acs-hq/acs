//! The canonical event every harness adapter produces.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::values::SpecVersion;

/// The moment in a session at which an event is raised.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Before the harness runs a tool. The engine's decision gates the call.
    PreTool,
    /// After the harness ran a tool. Used to attribute what actually changed.
    PostTool,
    /// A session started. No tool is involved.
    SessionStart,
    /// The user submitted a prompt. No tool is involved.
    PromptSubmit,
    /// A session ended. No tool is involved.
    SessionEnd,
}

/// What a tool does, as far as ACS is concerned. Adapters map each harness tool onto one kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    /// Reads files without changing anything.
    Read,
    /// Creates a file or overwrites it entirely.
    Write,
    /// Modifies part of an existing file.
    Edit,
    /// Deletes files.
    Delete,
    /// Moves or renames files. `paths` lists the source followed by the destination.
    Move,
    /// Runs a shell command. The command text is in `command`.
    Shell,
    /// Anything else (search, web fetch, MCP tools, ...).
    Other,
}

/// The tool a `pre_tool` or `post_tool` event refers to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Tool {
    /// The harness's own name for the tool, for example `Edit` or `Bash`.
    pub name: String,
    /// What the tool does, as classified by the adapter.
    pub kind: ToolKind,
}

/// The text change made by an edit tool. Enables symbol-level claims.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EditInfo {
    /// The text being replaced.
    pub old_text: String,
    /// The text it is replaced with.
    pub new_text: String,
}

/// The outcome of a tool call. Present on `post_tool` events only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ToolResult {
    /// Whether the tool call succeeded.
    pub success: bool,
    /// The exit code, for shell commands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

/// The canonical event: what every adapter produces from a harness payload.
///
/// `pre_tool` and `post_tool` events MUST carry `tool`; session events (`session_start`,
/// `prompt_submit`, `session_end`) do not. Unknown fields are ignored so that newer producers
/// remain readable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ToolEvent {
    /// The spec version this event conforms to. Always `acs/0.1`.
    pub spec: SpecVersion,
    /// Name of the harness that produced the event, for example `claude-code` or `antigravity`.
    /// A free-form string so new adapters need no schema change; `canonical` marks synthetic
    /// events (tests, the conformance suite).
    pub harness: String,
    /// The harness's own identifier for the session. One harness session is one seat.
    pub harness_session_id: String,
    /// When in the session this event was raised.
    pub phase: Phase,
    /// Absolute working directory of the harness process when the event was raised.
    pub cwd: String,
    /// The tool involved. Absent for session phases.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<Tool>,
    /// Paths the tool touches, exactly as the harness gave them: absolute, or relative to `cwd`.
    /// Normalization happens later in the pipeline.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    /// The text change, when the tool is an edit and the harness exposes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edit: Option<EditInfo>,
    /// The shell command text, when `tool.kind` is `shell`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// The tool's outcome. `post_tool` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<ToolResult>,
    /// The user's prompt. `prompt_submit` only. Implementations MUST NOT store it unless
    /// `ledger.store_prompts` is enabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// The original harness payload, kept for debugging only. Never interpreted.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub raw: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_session_event_parses_and_omits_empty_fields() {
        let json = r#"{"spec":"acs/0.1","harness":"canonical","harness_session_id":"s1","phase":"session_start","cwd":"/repo"}"#;
        let event: ToolEvent = serde_json::from_str(json).unwrap();
        assert!(event.tool.is_none() && event.paths.is_empty() && event.raw.is_null());
        assert_eq!(serde_json::to_string(&event).unwrap(), json);
    }

    #[test]
    fn unknown_fields_are_ignored_but_unknown_enum_values_are_not() {
        let base = |extra: &str, kind: &str| {
            format!(
                r#"{{"spec":"acs/0.1","harness":"h","harness_session_id":"s","phase":"pre_tool","cwd":"/r",
                "tool":{{"name":"T","kind":"{kind}"}}{extra}}}"#
            )
        };
        assert!(serde_json::from_str::<ToolEvent>(&base(r#","future_field":1"#, "edit")).is_ok());
        assert!(serde_json::from_str::<ToolEvent>(&base("", "teleport")).is_err());
    }
}
