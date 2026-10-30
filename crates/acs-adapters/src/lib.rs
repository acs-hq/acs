//! ACS adapters: harness payloads to and from canonical events and decisions.
//!
//! An [`Adapter`] translates between one harness's hook protocol and ACS's canonical types:
//! [`Adapter::parse`] turns the bytes the harness wrote to the hook's stdin into a
//! [`ToolEvent`], and [`Adapter::render`] turns the engine's [`Decision`] into the bytes (and exit
//! code) the harness expects. Adapters are pure: no I/O, no state, no store access. The hook
//! pipeline in `acs-cli` does everything in between.
//!
//! The registry ([`by_name`]) maps the harness name given to `acs hook <harness>` to its adapter.
//! This crate ships the [`canonical`] adapter, whose stdin is a `ToolEvent` and whose stdout is a
//! `Decision` (used by tests, the conformance suite and harnesses that speak the spec directly).
//! A harness adapter adds one module and one entry to [`ADAPTERS`].

use acs_core::{Decision, Phase, ToolEvent};

pub mod antigravity;

/// What the hook process should write and exit with.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HookOutput {
    /// Bytes for the process's stdout (the harness reads the decision here).
    pub stdout: Vec<u8>,
    /// Bytes for the process's stderr (some harnesses show it to the agent when exit code is 2).
    pub stderr: Vec<u8>,
    /// The process exit code.
    pub exit_code: i32,
}

/// A payload an adapter could not parse into a [`ToolEvent`].
///
/// Carries whatever could still be recovered from the payload, so the pipeline can log the
/// problem against the right session and repository (SPEC section 11: unknown payloads are
/// allowed, logged, and warned about once per session).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterError {
    /// What was wrong with the payload.
    pub message: String,
    /// The harness's session id, if the payload had one.
    pub harness_session_id: Option<String>,
    /// The working directory, if the payload had one.
    pub cwd: Option<String>,
}

impl AdapterError {
    /// An error with nothing recovered.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            harness_session_id: None,
            cwd: None,
        }
    }

    /// An error that recovers `harness_session_id` and `cwd` from `payload` when it is a JSON
    /// object holding them as strings under those names (or, for the session id, `session_id`).
    #[must_use]
    pub fn recovering(message: impl Into<String>, payload: &[u8]) -> Self {
        let mut error = Self::new(message);
        if let Ok(serde_json::Value::Object(map)) = serde_json::from_slice(payload) {
            let text = |key: &str| {
                map.get(key)
                    .and_then(serde_json::Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
            };
            error.harness_session_id = text("harness_session_id").or_else(|| text("session_id"));
            error.cwd = text("cwd");
        }
        error
    }
}

impl std::fmt::Display for AdapterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for AdapterError {}

/// Translates one harness's hook protocol.
///
/// Implementations are stateless statics, so the trait requires [`Sync`].
pub trait Adapter: Sync {
    /// The harness name, as given to `acs hook <name>` (lowercase, for example `claude-code`).
    fn name(&self) -> &'static str;

    /// Parses the bytes the harness wrote to stdin.
    ///
    /// `phase` is the value of `--phase` when the hook command line gives one (harnesses that
    /// call a different command per phase); adapters whose payload names the phase itself ignore
    /// it. Classification must be conservative: a tool that may change files and cannot be
    /// classified is `shell` or `other`, never `read`.
    ///
    /// # Errors
    ///
    /// [`AdapterError`] when the payload is not one this adapter understands.
    fn parse(&self, phase: Option<Phase>, payload: &[u8]) -> Result<ToolEvent, AdapterError>;

    /// Renders the decision the way the harness expects.
    ///
    /// `event` is the parsed event, or `None` when parsing failed (the decision is then the
    /// `unknown_payload` allow, which must still be rendered as an allow). `deny` must block the
    /// call and show `decision.message` to the agent; `ask` must prompt the human, or be treated
    /// as `deny` by a harness that cannot; `warnings` and `context` are shown to the agent as
    /// added context where the harness supports it.
    fn render(&self, event: Option<&ToolEvent>, decision: &Decision) -> HookOutput;
}

/// Every adapter, in the order `acs hook --help` lists them. A harness adapter adds one entry.
static ADAPTERS: &[&dyn Adapter] = &[
    &canonical::Canonical,
    &claude_code::ClaudeCode,
    &antigravity::Antigravity,
];

/// The adapter for `name` (case-insensitive), if there is one.
#[must_use]
pub fn by_name(name: &str) -> Option<&'static dyn Adapter> {
    ADAPTERS
        .iter()
        .copied()
        .find(|adapter| adapter.name().eq_ignore_ascii_case(name))
}

/// The names of all adapters.
#[must_use]
pub fn names() -> Vec<&'static str> {
    ADAPTERS.iter().map(|adapter| adapter.name()).collect()
}

pub mod claude_code;

pub mod canonical {
    //! The canonical adapter: stdin is a [`ToolEvent`] as JSON, stdout is a [`Decision`] as JSON,
    //! the exit code is always 0 (the decision is in the document, not in the exit status).

    use acs_core::{Decision, Phase, ToolEvent};

    use super::{Adapter, AdapterError, HookOutput};

    /// The adapter registered as `canonical`.
    #[derive(Debug, Clone, Copy, Default)]
    pub struct Canonical;

    impl Adapter for Canonical {
        fn name(&self) -> &'static str {
            "canonical"
        }

        /// Parses a `ToolEvent`. A given `phase` overrides the event's own `phase`: the command
        /// line (`acs hook --canonical pre_tool`) is authoritative, which lets one event
        /// document be replayed through several phases.
        fn parse(&self, phase: Option<Phase>, payload: &[u8]) -> Result<ToolEvent, AdapterError> {
            let mut event: ToolEvent = serde_json::from_slice(payload).map_err(|err| {
                AdapterError::recovering(format!("not a ToolEvent: {err}"), payload)
            })?;
            if let Some(phase) = phase {
                event.phase = phase;
            }
            Ok(event)
        }

        fn render(&self, _event: Option<&ToolEvent>, decision: &Decision) -> HookOutput {
            // Serializing a `Decision` cannot fail (strings and enums only); fall back to an
            // empty object rather than panicking.
            let mut stdout = serde_json::to_vec(decision).unwrap_or_else(|_| b"{}".to_vec());
            stdout.push(b'\n');
            HookOutput {
                stdout,
                stderr: Vec::new(),
                exit_code: 0,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use acs_core::{Outcome, ReasonCode};

    use super::*;

    const EVENT: &str = r#"{"spec":"acs/0.1","harness":"canonical","harness_session_id":"s1","phase":"pre_tool","cwd":"/repo","tool":{"name":"Edit","kind":"edit"},"paths":["src/a.rs"]}"#;

    #[test]
    fn registry_finds_canonical_case_insensitively() {
        assert_eq!(by_name("canonical").map(|a| a.name()), Some("canonical"));
        assert_eq!(by_name("Canonical").map(|a| a.name()), Some("canonical"));
        assert!(by_name("no-such-harness").is_none());
        assert_eq!(names(), vec!["canonical", "claude-code", "antigravity"]);
    }

    #[test]
    fn canonical_parses_a_tool_event_and_the_phase_argument_wins() {
        let adapter = by_name("canonical").unwrap();
        let event = adapter.parse(None, EVENT.as_bytes()).unwrap();
        assert_eq!(event.phase, Phase::PreTool);
        assert_eq!(event.paths, vec!["src/a.rs"]);
        let event = adapter
            .parse(Some(Phase::PostTool), EVENT.as_bytes())
            .unwrap();
        assert_eq!(event.phase, Phase::PostTool);
    }

    #[test]
    fn canonical_failures_recover_session_id_and_cwd() {
        let adapter = by_name("canonical").unwrap();
        let err = adapter
            .parse(
                None,
                br#"{"harness_session_id":"s9","cwd":"/work","phase":"teleport"}"#,
            )
            .unwrap_err();
        assert_eq!(err.harness_session_id.as_deref(), Some("s9"));
        assert_eq!(err.cwd.as_deref(), Some("/work"));
        assert!(err.to_string().starts_with("not a ToolEvent"));

        let err = adapter.parse(None, b"garbage").unwrap_err();
        assert_eq!(err.harness_session_id, None);
        assert_eq!(err.cwd, None);
        let err = AdapterError::recovering("x", br#"{"session_id":"claude-1"}"#);
        assert_eq!(err.harness_session_id.as_deref(), Some("claude-1"));
    }

    #[test]
    fn canonical_renders_the_decision_as_json_with_exit_zero() {
        let adapter = by_name("canonical").unwrap();
        let decision = Decision::new(Outcome::Deny, ReasonCode::ClaimConflict, "held by sara");
        let out = adapter.render(None, &decision);
        assert_eq!(out.exit_code, 0);
        assert!(out.stderr.is_empty());
        assert!(out.stdout.ends_with(b"\n"));
        let back: Decision = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(back, decision);
    }
}
