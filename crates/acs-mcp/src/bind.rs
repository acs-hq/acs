//! What the protocol layer can tell the backend about the harness session behind a tool call
//! (the harness-specific half of decision 0011 section 5; the backend does the seat lookup).
//!
//! | Harness | Session id source | Per call |
//! | --- | --- | --- |
//! | Claude Code | `CLAUDE_CODE_SESSION_ID` in the server's environment (stale after `--continue` and `/clear`, so the backend validates it) | `_meta["claudecode/toolUseId"]` |
//! | Antigravity | `_meta["antigravity.google/root_conversation_id"]` on every `tools/call` (since 1.4.4); nothing at `initialize` or in the environment | |
//! | other | none | |

use serde_json::{Map, Value};

use crate::Caller;

/// The harness name of Claude Code (the name its adapter registers).
pub const CLAUDE_CODE: &str = "claude-code";
/// The harness name of Antigravity.
pub const ANTIGRAVITY: &str = "antigravity";

/// `params._meta` key with the top-level conversation id on Antigravity (1.4.4 and later).
pub const ANTIGRAVITY_ROOT_KEY: &str = "antigravity.google/root_conversation_id";
/// `params._meta` key with the tool use id on Claude Code.
pub const CLAUDE_TOOL_USE_KEY: &str = "claudecode/toolUseId";
/// `params._meta` key with the client's identity on every request of the stateless protocol
/// revision `2026-07-28` (Claude Code 2.1.314 speaks it: no `initialize`, a `server/discover`
/// probe, and `{name, version, ...}` here instead).
pub const CLIENT_INFO_KEY: &str = "io.modelcontextprotocol/clientInfo";
/// The environment variable Claude Code sets for the MCP servers it starts.
pub const CLAUDE_SESSION_ENV: &str = "CLAUDE_CODE_SESSION_ID";

/// What the server was told at start-up.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BindingSettings {
    /// `--harness`, when given.
    pub harness: Option<String>,
    /// The value of [`CLAUDE_SESSION_ENV`] in the server's environment.
    pub claude_code_session_id: Option<String>,
}

impl BindingSettings {
    /// Settings from the process environment and an optional `--harness`.
    #[must_use]
    pub fn from_process(harness: Option<String>) -> Self {
        Self {
            harness,
            claude_code_session_id: std::env::var(CLAUDE_SESSION_ENV).ok(),
        }
    }

    /// The [`Caller`] of one `tools/call`: `client_name` is `clientInfo.name` from `initialize`
    /// (when the client sent one; stateless clients put it in `meta` instead), `meta` the
    /// request's `_meta` object (empty when there is none).
    ///
    /// The harness is, in order: `--harness`; Antigravity if its conversation id is in `_meta`;
    /// Claude Code if its tool use id is in `_meta` or its session id is in the environment; the
    /// client name when it names a known harness; else unknown. The session id is taken from
    /// the source of the *chosen* harness only.
    #[must_use]
    pub fn caller(&self, client_name: Option<&str>, meta: &Map<String, Value>) -> Caller {
        let antigravity_id = meta_str(meta, ANTIGRAVITY_ROOT_KEY);
        let tool_use_id = meta_str(meta, CLAUDE_TOOL_USE_KEY);
        let claude_id = non_empty(self.claude_code_session_id.as_deref());
        let client_name = client_name.or_else(|| {
            meta.get(CLIENT_INFO_KEY)
                .and_then(|info| info.get("name"))
                .and_then(Value::as_str)
        });

        let harness = non_empty(self.harness.as_deref())
            .map(str::to_ascii_lowercase)
            .or_else(|| {
                if antigravity_id.is_some() {
                    Some(ANTIGRAVITY.to_owned())
                } else if tool_use_id.is_some() || claude_id.is_some() {
                    Some(CLAUDE_CODE.to_owned())
                } else {
                    client_name.and_then(harness_from_client_name)
                }
            });
        let session_id = match harness.as_deref() {
            Some(CLAUDE_CODE) => claude_id,
            Some(ANTIGRAVITY) => antigravity_id,
            _ => None,
        };
        let tool_use_id = tool_use_id
            .filter(|_| harness.as_deref() == Some(CLAUDE_CODE))
            .map(str::to_owned);
        Caller {
            harness,
            session_id: session_id.map(str::to_owned),
            tool_use_id,
        }
    }
}

fn harness_from_client_name(name: &str) -> Option<String> {
    let name = name.to_ascii_lowercase();
    if name.contains("claude") {
        Some(CLAUDE_CODE.to_owned())
    } else if name.contains("antigravity") {
        Some(ANTIGRAVITY.to_owned())
    } else {
        None
    }
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|v| !v.is_empty())
}

fn meta_str<'a>(meta: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    non_empty(meta.get(key).and_then(Value::as_str))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn meta(value: Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap_or_default()
    }

    fn settings(harness: Option<&str>, env: Option<&str>) -> BindingSettings {
        BindingSettings {
            harness: harness.map(str::to_owned),
            claude_code_session_id: env.map(str::to_owned),
        }
    }

    #[test]
    fn claude_code_uses_the_environment_session_and_the_tool_use_id() {
        let caller = settings(Some("claude-code"), Some("sess-1")).caller(
            Some("claude-code"),
            &meta(json!({"claudecode/toolUseId": "toolu_9"})),
        );
        assert_eq!(caller.harness.as_deref(), Some("claude-code"));
        assert_eq!(caller.session_id.as_deref(), Some("sess-1"));
        assert_eq!(caller.tool_use_id.as_deref(), Some("toolu_9"));
    }

    #[test]
    fn antigravity_uses_the_root_conversation_id_per_call() {
        let s = settings(Some("antigravity"), None);
        let first = s.caller(
            None,
            &meta(json!({"antigravity.google/root_conversation_id": "conv-a"})),
        );
        let second = s.caller(
            None,
            &meta(json!({"antigravity.google/root_conversation_id": "conv-b"})),
        );
        assert_eq!(first.session_id.as_deref(), Some("conv-a"));
        assert_eq!(second.session_id.as_deref(), Some("conv-b"));
        // Older Antigravity builds send no id: the backend falls back.
        let none = s.caller(None, &Map::new());
        assert_eq!(none.harness.as_deref(), Some("antigravity"));
        assert_eq!(none.session_id, None);
    }

    #[test]
    fn the_harness_is_inferred_without_the_flag() {
        let s = settings(None, None);
        let ag = s.caller(
            None,
            &meta(json!({"antigravity.google/root_conversation_id": "c"})),
        );
        assert_eq!(ag.harness.as_deref(), Some("antigravity"));
        let cc = settings(None, Some("s")).caller(None, &Map::new());
        assert_eq!(cc.harness.as_deref(), Some("claude-code"));
        assert_eq!(cc.session_id.as_deref(), Some("s"));
        let by_name = s.caller(Some("Claude-Code"), &Map::new());
        assert_eq!(by_name.harness.as_deref(), Some("claude-code"));
        assert_eq!(by_name.session_id, None);
        let unknown = s.caller(Some("inspector"), &Map::new());
        assert_eq!(unknown, Caller::default());
    }

    #[test]
    fn the_real_claude_code_requests_bind_from_the_environment() {
        // Captured from Claude Code 2.1.314 (`claude -p`, MCP server registered with
        // `--mcp-config`): no `initialize`; the client identity and the tool use id ride in
        // each request's `_meta`.
        let captured =
            include_str!("../../../tests/fixtures/claude-code/mcp_requests.captured.jsonl");
        let calls: Vec<Value> = captured
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(|v| v["method"] == "tools/call")
            .collect();
        assert_eq!(calls.len(), 2);
        for call in &calls {
            let meta = call["params"]["_meta"]
                .as_object()
                .cloned()
                .unwrap_or_default();
            // Without `--harness` and without the environment variable, the client name decides.
            let by_name = settings(None, None).caller(None, &meta);
            assert_eq!(by_name.harness.as_deref(), Some("claude-code"));
            assert_eq!(by_name.session_id, None);
            assert!(
                by_name
                    .tool_use_id
                    .as_deref()
                    .is_some_and(|id| id.starts_with("toolu_")),
                "{by_name:?}"
            );
            let bound = settings(Some("claude-code"), Some("5e6f")).caller(None, &meta);
            assert_eq!(bound.session_id.as_deref(), Some("5e6f"));
        }
    }

    #[test]
    fn the_documented_antigravity_call_binds_from_its_meta() {
        let documented: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/antigravity/mcp_tools_call.documented.json"
        ))
        .unwrap_or_default();
        let meta = documented["params"]["_meta"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        let caller = settings(None, None).caller(None, &meta);
        assert_eq!(caller.harness.as_deref(), Some("antigravity"));
        assert_eq!(
            caller.session_id.as_deref(),
            Some("0b8f6c1e-2d4a-4e7b-9a3c-5f1d2e8a7b60")
        );
    }

    #[test]
    fn a_hint_of_another_harness_is_ignored() {
        // The flag wins: the Claude Code variable does not name an Antigravity session.
        let caller = settings(Some("antigravity"), Some("sess-1")).caller(None, &Map::new());
        assert_eq!(caller.session_id, None);
        let caller = settings(Some("claude-code"), None).caller(
            None,
            &meta(json!({"antigravity.google/root_conversation_id": "c"})),
        );
        assert_eq!(caller.session_id, None);
    }

    #[test]
    fn blank_values_are_absent() {
        let caller = settings(Some(" "), Some("")).caller(
            None,
            &meta(
                json!({"claudecode/toolUseId": "", "antigravity.google/root_conversation_id": 7}),
            ),
        );
        assert_eq!(caller, Caller::default());
    }
}
