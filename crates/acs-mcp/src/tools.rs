//! The six tools: their definitions (written for agents: short and concrete), argument
//! parsing, dispatch to a [`Backend`], and the text of the results.
//!
//! This module knows nothing about the MCP protocol library: [`definitions`] and [`call`] work on
//! plain JSON values, so they are tested with a fake backend. Failures are never protocol errors:
//! a bad argument, a missing session or a refused claim is a [`ToolOutput`] with `is_error` set
//! and text that says what to do next.

use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};

use crate::backend::{
    Backend, BackendError, Caller, ClaimInfo, ClaimReply, ClaimRequest, DecisionInfo, IntentReply,
    IntentRequest, LoreProposeReply, LoreProposeRequest, ReleaseReply, ReleaseRequest, StatusReply,
    WhyReply,
};

/// `acs_status`.
pub const STATUS: &str = "acs_status";
/// `acs_claim`.
pub const CLAIM: &str = "acs_claim";
/// `acs_release`.
pub const RELEASE: &str = "acs_release";
/// `acs_intent`.
pub const INTENT: &str = "acs_intent";
/// `acs_why`.
pub const WHY: &str = "acs_why";
/// `acs_lore_propose`.
pub const LORE_PROPOSE: &str = "acs_lore_propose";

/// How many items of a long list a text result shows before "and N more".
const LIST_LIMIT: usize = 12;

/// A tool as listed by `tools/list`.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolDef {
    /// The tool name.
    pub name: &'static str,
    /// What an agent reads when choosing a tool: short and concrete.
    pub description: &'static str,
    /// JSON Schema of the arguments.
    pub input_schema: Map<String, Value>,
    /// The tool changes nothing.
    pub read_only: bool,
    /// Calling it again with the same arguments changes nothing more.
    pub idempotent: bool,
}

/// What a tool call returns.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutput {
    /// The text for the agent.
    pub text: String,
    /// The same result as JSON (the backend's reply), when there is one.
    pub structured: Option<Value>,
    /// The action did not happen (or failed); the text says what to do.
    pub is_error: bool,
}

impl ToolOutput {
    fn ok(text: String, structured: &impl serde::Serialize) -> Self {
        Self {
            text,
            structured: serde_json::to_value(structured).ok(),
            is_error: false,
        }
    }

    fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            structured: None,
            is_error: true,
        }
    }
}

// Arguments of `acs_claim` and `acs_release`. (Plain comments: doc comments on the types would
// end up in the schema sent to the model.)
#[derive(Debug, Deserialize, JsonSchema)]
struct PathsArgs {
    /// Files or directories, relative to the repository root (or absolute). A directory covers
    /// the files under it.
    #[serde(deserialize_with = "one_or_many")]
    #[schemars(with = "Vec<String>")]
    paths: Vec<String>,
}

// Arguments of `acs_intent`.
#[derive(Debug, Deserialize, JsonSchema)]
struct IntentArgs {
    /// One sentence: what you are about to do and which files it touches.
    text: String,
}

// Arguments of `acs_lore_propose`.
#[derive(Debug, Deserialize, JsonSchema)]
struct LoreProposeArgs {
    /// Path globs relative to the repository root that the fact applies to, such as
    /// ["payments/**"]. Leave it out when replacing an entry to keep that entry's scope.
    #[serde(default, deserialize_with = "one_or_many")]
    #[schemars(with = "Vec<String>")]
    scope: Vec<String>,
    /// The fact, in a few sentences of Markdown: what the next agent working there must know
    /// that the code does not make obvious.
    text: String,
    /// The id of a lore entry you were shown (such as lore_7f3a) that this corrects.
    #[serde(default)]
    replaces: Option<String>,
    /// One sentence for the reviewer: how you learned it.
    #[serde(default)]
    why: Option<String>,
}

// Arguments of `acs_status` and `acs_why`: none.
#[derive(Debug, Deserialize, JsonSchema)]
struct NoArgs {}

/// Agents sometimes pass one path as a string; accept it.
fn one_or_many<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Paths {
        One(String),
        Many(Vec<String>),
    }
    Ok(match Paths::deserialize(deserializer)? {
        Paths::One(path) => vec![path],
        Paths::Many(paths) => paths,
    })
}

fn schema_of<T: JsonSchema>() -> Map<String, Value> {
    let mut schema = match serde_json::to_value(schemars::schema_for!(T)) {
        Ok(Value::Object(map)) => map,
        _ => Map::new(),
    };
    // Some clients reject the meta-schema URL; the arguments are plain draft 2020-12.
    schema.remove("$schema");
    schema.remove("title");
    schema.remove("description");
    schema
        .entry("properties")
        .or_insert_with(|| Value::Object(Map::new()));
    schema
}

/// The six tool definitions, in the order they are listed.
#[must_use]
pub fn definitions() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: STATUS,
            description: "Show what you have claimed, what other agents have claimed, and which \
                          paths are never writable. Call it at the start of a task and before \
                          editing files others may be working on.",
            input_schema: schema_of::<NoArgs>(),
            read_only: true,
            idempotent: true,
        },
        ToolDef {
            name: CLAIM,
            description: "Claim files before you change them, so no other agent edits them at \
                          the same time. Deletes (and moves) outside your own claims are denied \
                          by default: claim a file before you delete it. For a large change, \
                          claim every file you will touch up front; a directory claims the files \
                          under it. If a file is taken, the result names the holder.",
            input_schema: schema_of::<PathsArgs>(),
            read_only: false,
            idempotent: true,
        },
        ToolDef {
            name: RELEASE,
            description: "Release your claims on files you are finished with, so others can \
                          edit them. Claims also end when your session ends.",
            input_schema: schema_of::<PathsArgs>(),
            read_only: false,
            idempotent: true,
        },
        ToolDef {
            name: INTENT,
            description: "Say in one sentence what you are about to do. Other agents and the \
                          user see it in acs_status. Call it when you start a task and when \
                          your plan changes.",
            input_schema: schema_of::<IntentArgs>(),
            read_only: false,
            idempotent: true,
        },
        ToolDef {
            name: WHY,
            description: "Explain the last decision ACS made for you, such as why an edit, a \
                          delete or a command was denied, and what to do instead.",
            input_schema: schema_of::<NoArgs>(),
            read_only: true,
            idempotent: true,
        },
        ToolDef {
            name: LORE_PROPOSE,
            description: "Propose lore: a short fact about part of this repository that the \
                          next agent working there needs and the code does not make obvious (a \
                          convention, a trap, a reason). The owner reviews it before anyone is \
                          shown it. Call it at the end of a task in which you learned such a \
                          fact, and to correct lore you were shown (pass its id as `replaces`). \
                          Never write files under .acs/ yourself.",
            input_schema: schema_of::<LoreProposeArgs>(),
            read_only: false,
            idempotent: false,
        },
    ]
}

/// The names of the tools, for messages.
#[must_use]
pub fn names() -> Vec<&'static str> {
    vec![STATUS, CLAIM, RELEASE, INTENT, WHY, LORE_PROPOSE]
}

/// Runs the tool `name` with `arguments` for `caller` against `backend`.
#[must_use]
pub fn call(
    backend: &dyn Backend,
    caller: &Caller,
    name: &str,
    arguments: Option<Map<String, Value>>,
) -> ToolOutput {
    let args = Value::Object(arguments.unwrap_or_default());
    match name {
        STATUS => status(backend, caller, &args),
        CLAIM => with_args::<PathsArgs>(name, args, |a| claim(backend, caller, a.paths)),
        RELEASE => with_args::<PathsArgs>(name, args, |a| release(backend, caller, a.paths)),
        INTENT => with_args::<IntentArgs>(name, args, |a| intent(backend, caller, a.text)),
        WHY => why(backend, caller, &args),
        LORE_PROPOSE => with_args::<LoreProposeArgs>(name, args, |a| {
            lore_propose(
                backend,
                caller,
                LoreProposeRequest {
                    scope: a.scope,
                    text: a.text,
                    replaces: a.replaces,
                    why: a.why,
                },
            )
        }),
        other => ToolOutput::error(format!(
            "Unknown tool `{other}`. The ACS tools are: {}.",
            names().join(", ")
        )),
    }
}

fn with_args<T: DeserializeOwned>(
    name: &str,
    args: Value,
    run: impl FnOnce(T) -> ToolOutput,
) -> ToolOutput {
    match serde_json::from_value::<T>(args) {
        Ok(parsed) => run(parsed),
        Err(err) => ToolOutput::error(format!(
            "Bad arguments for {name}: {err}. {}",
            usage_of(name)
        )),
    }
}

fn usage_of(name: &str) -> &'static str {
    match name {
        CLAIM | RELEASE => "Pass `paths`: a list of file or directory paths.",
        INTENT => "Pass `text`: one sentence saying what you are about to do.",
        LORE_PROPOSE => {
            "Pass `text` (the fact) and `scope` (path globs such as [\"payments/**\"]); \
             optionally `replaces` (a lore id) and `why`."
        }
        _ => "This tool takes no arguments.",
    }
}

fn status(backend: &dyn Backend, caller: &Caller, _args: &Value) -> ToolOutput {
    match backend.status(caller) {
        Ok(reply) => ToolOutput::ok(status_text(&reply), &reply),
        Err(err) => failure(&err),
    }
}

fn claim(backend: &dyn Backend, caller: &Caller, paths: Vec<String>) -> ToolOutput {
    if paths.iter().all(|p| p.trim().is_empty()) {
        return ToolOutput::error(format!("Nothing to claim. {}", usage_of(CLAIM)));
    }
    match backend.claim(caller, &ClaimRequest { paths }) {
        Ok(reply) => {
            let refused = matches!(reply, ClaimReply::Refused { .. });
            let mut out = ToolOutput::ok(claim_text(&reply), &reply);
            out.is_error = refused;
            out
        }
        Err(err) => failure(&err),
    }
}

fn release(backend: &dyn Backend, caller: &Caller, paths: Vec<String>) -> ToolOutput {
    if paths.iter().all(|p| p.trim().is_empty()) {
        return ToolOutput::error(format!("Nothing to release. {}", usage_of(RELEASE)));
    }
    match backend.release(caller, &ReleaseRequest { paths }) {
        Ok(reply) => ToolOutput::ok(release_text(&reply), &reply),
        Err(err) => failure(&err),
    }
}

fn intent(backend: &dyn Backend, caller: &Caller, text: String) -> ToolOutput {
    if text.trim().is_empty() {
        return ToolOutput::error(format!("The intent is empty. {}", usage_of(INTENT)));
    }
    match backend.intent(caller, &IntentRequest { text }) {
        Ok(reply) => ToolOutput::ok(intent_text(&reply), &reply),
        Err(err) => failure(&err),
    }
}

fn why(backend: &dyn Backend, caller: &Caller, _args: &Value) -> ToolOutput {
    match backend.why(caller) {
        Ok(reply) => ToolOutput::ok(why_text(&reply), &reply),
        Err(err) => failure(&err),
    }
}

fn lore_propose(backend: &dyn Backend, caller: &Caller, request: LoreProposeRequest) -> ToolOutput {
    if request.text.trim().is_empty() {
        return ToolOutput::error(format!(
            "The lore text is empty. {}",
            usage_of(LORE_PROPOSE)
        ));
    }
    match backend.lore_propose(caller, &request) {
        Ok(reply) => ToolOutput::ok(lore_propose_text(&reply), &reply),
        Err(err) => failure(&err),
    }
}

fn failure(err: &BackendError) -> ToolOutput {
    ToolOutput::error(err.to_string())
}

// ----------------------------------------------------------------------------------------
// Text of the results.

fn ticked(items: &[String], limit: usize) -> String {
    let mut text = items
        .iter()
        .take(limit)
        .map(|item| format!("`{item}`"))
        .collect::<Vec<_>>()
        .join(", ");
    if items.len() > limit {
        text.push_str(&format!(" and {} more", items.len() - limit));
    }
    text
}

fn holder(claim: &ClaimInfo) -> String {
    match &claim.harness {
        Some(harness) => format!("{}, {harness}", claim.holder),
        None => claim.holder.clone(),
    }
}

fn status_text(reply: &StatusReply) -> String {
    let mut lines = Vec::new();
    let branch = reply
        .branch
        .as_deref()
        .map_or("(detached HEAD)".to_owned(), |b| format!("`{b}`"));
    lines.push(format!(
        "Repository {}, branch {branch}, merging into `{}`.",
        reply.worktree, reply.target_branch
    ));
    match &reply.seat {
        Some(seat) => lines.push(format!(
            "You: {} session {}, pilot {}. Intent: {}.",
            seat.harness,
            seat.session_id,
            seat.pilot,
            seat.intent
                .as_deref()
                .map_or("not set (call acs_intent)".to_owned(), |i| format!(
                    "\"{i}\""
                )),
        )),
        None => lines.push("You: no active ACS session yet.".to_owned()),
    }
    if reply.gate_off {
        lines.push(
            "ACS enforcement is off (`acs off`): claims are not enforced. Sealed paths still are."
                .to_owned(),
        );
    }
    if reply.my_claims.is_empty() {
        lines.push("Your claims: none.".to_owned());
    } else {
        let paths: Vec<String> = reply.my_claims.iter().map(|c| c.path.clone()).collect();
        lines.push(format!(
            "Your claims ({}): {}.",
            paths.len(),
            ticked(&paths, LIST_LIMIT)
        ));
    }
    if reply.other_claims.is_empty() {
        lines.push("Claimed by others: nothing.".to_owned());
    } else {
        lines.push(format!("Claimed by others ({}):", reply.other_claims.len()));
        for claim in reply.other_claims.iter().take(LIST_LIMIT) {
            lines.push(format!("- `{}` ({})", claim.path, holder(claim)));
        }
        if reply.other_claims.len() > LIST_LIMIT {
            lines.push(format!(
                "- and {} more",
                reply.other_claims.len() - LIST_LIMIT
            ));
        }
    }
    if !reply.sealed.is_empty() {
        lines.push(format!(
            "Never writable: {}.",
            ticked(&reply.sealed, LIST_LIMIT)
        ));
    }
    for note in &reply.notes {
        lines.push(format!("Note: {note}"));
    }
    lines.join("\n")
}

fn claim_text(reply: &ClaimReply) -> String {
    match reply {
        ClaimReply::Granted {
            paths,
            warnings,
            lore,
        } => {
            let mut text = if paths.is_empty() {
                "Claimed nothing new.".to_owned()
            } else {
                format!(
                    "Claimed {} file{}: {}. Other agents cannot edit {} until you release {} \
                     (acs_release) or your session ends. You may delete {} too.",
                    paths.len(),
                    plural(paths.len()),
                    ticked(paths, LIST_LIMIT),
                    pronoun(paths.len(), "it", "them"),
                    pronoun(paths.len(), "it", "them"),
                    pronoun(paths.len(), "it", "them"),
                )
            };
            for warning in warnings {
                text.push_str(&format!("\nWarning: {warning}"));
            }
            for block in lore {
                text.push_str("\n\n");
                text.push_str(block);
            }
            text
        }
        ClaimReply::Refused {
            reason_code,
            message,
        } => format!("Nothing was claimed ({reason_code}). {message}"),
    }
}

fn release_text(reply: &ReleaseReply) -> String {
    let mut parts = Vec::new();
    if !reply.released.is_empty() {
        parts.push(format!(
            "Released: {}.",
            ticked(&reply.released, LIST_LIMIT)
        ));
    }
    if !reply.not_held.is_empty() {
        parts.push(format!(
            "You held no claim on: {}.",
            ticked(&reply.not_held, LIST_LIMIT)
        ));
    }
    if !reply.outside.is_empty() {
        parts.push(format!(
            "Outside the repository, ignored: {}.",
            ticked(&reply.outside, LIST_LIMIT)
        ));
    }
    if reply.released.is_empty() {
        parts.insert(0, "Nothing was released.".to_owned());
    }
    parts.join(" ")
}

fn intent_text(reply: &IntentReply) -> String {
    format!(
        "Intent recorded: \"{}\". Other agents and the user see it in acs_status.",
        reply.intent
    )
}

fn why_text(reply: &WhyReply) -> String {
    let Some(DecisionInfo {
        outcome,
        reason_code,
        message,
        at,
    }) = &reply.decision
    else {
        return "ACS has made no decision for you yet: nothing has been denied.".to_owned();
    };
    match (outcome.as_str(), message.trim().is_empty()) {
        ("allow", true) => {
            format!("Your last action was allowed ({reason_code}, {at}). Nothing was blocked.")
        }
        ("allow", false) => {
            format!("Your last action was allowed with a note ({reason_code}, {at}): {message}")
        }
        _ => format!("Your last action was {outcome} ({reason_code}, {at}).\n{message}"),
    }
}

fn lore_propose_text(reply: &LoreProposeReply) -> String {
    let what = match &reply.replaces {
        Some(id) => format!("a correction of `{id}`"),
        None => "new lore".to_owned(),
    };
    let reviewer = reply
        .reviewer
        .as_deref()
        .map_or_else(|| "The owner".to_owned(), |r| format!("Its owner ({r})"));
    format!(
        "Proposed {what} for {} as {}. {reviewer} reviews it with `acs lore review`; it is not \
         shown to anyone until accepted. Nothing else to do.",
        ticked(&reply.scope, LIST_LIMIT),
        reply.proposal_id
    )
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

fn pronoun(n: usize, one: &'static str, many: &'static str) -> &'static str {
    if n == 1 { one } else { many }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use serde_json::json;

    use super::*;
    use crate::backend::SeatInfo;

    /// A backend with canned replies that records the requests it sees.
    #[derive(Default)]
    struct Fake {
        calls: Mutex<Vec<String>>,
        no_session: bool,
        refuse: bool,
    }

    impl Fake {
        fn note(&self, what: String) {
            if let Ok(mut calls) = self.calls.lock() {
                calls.push(what);
            }
        }
        fn gate(&self) -> Result<(), BackendError> {
            if self.no_session {
                Err(BackendError::NoSession(
                    "no active ACS session yet — your first file edit will start one".to_owned(),
                ))
            } else {
                Ok(())
            }
        }
    }

    impl Backend for Fake {
        fn status(&self, caller: &Caller) -> Result<StatusReply, BackendError> {
            self.note(format!("status {:?}", caller.session_id));
            Ok(StatusReply {
                worktree: "/repo".into(),
                branch: Some("feature".into()),
                target_branch: "main".into(),
                seat: (!self.no_session).then(|| SeatInfo {
                    seat_id: "seat_1".into(),
                    session_id: "ses_1".into(),
                    harness: "claude-code".into(),
                    pilot: "sara@example.com".into(),
                    intent: None,
                }),
                my_claims: vec![ClaimInfo {
                    path: "src/a.rs".into(),
                    symbol_path: None,
                    holder: "sara@example.com".into(),
                    harness: None,
                    source: "local".into(),
                    expires_at: "2026-01-01T00:00:00Z".into(),
                }],
                other_claims: vec![ClaimInfo {
                    path: "src/b.rs".into(),
                    symbol_path: None,
                    holder: "omar@example.com".into(),
                    harness: Some("antigravity".into()),
                    source: "remote".into(),
                    expires_at: "2026-01-01T00:00:00Z".into(),
                }],
                sealed: vec![".env".into()],
                notes: if self.no_session {
                    vec!["no active ACS session yet".into()]
                } else {
                    vec![]
                },
                ..StatusReply::default()
            })
        }

        fn claim(
            &self,
            _caller: &Caller,
            request: &ClaimRequest,
        ) -> Result<ClaimReply, BackendError> {
            self.gate()?;
            self.note(format!("claim {:?}", request.paths));
            Ok(if self.refuse {
                ClaimReply::Refused {
                    reason_code: "claim_conflict".into(),
                    message: "`src/b.rs` is claimed by omar@example.com (antigravity).".into(),
                }
            } else {
                ClaimReply::Granted {
                    paths: request.paths.clone(),
                    warnings: vec!["the remote view is stale".into()],
                    lore: vec![
                        "ACS lore for `src/**` (lore_a, owner sara@example.com):
Use X."
                            .into(),
                    ],
                }
            })
        }

        fn release(
            &self,
            _caller: &Caller,
            request: &ReleaseRequest,
        ) -> Result<ReleaseReply, BackendError> {
            self.gate()?;
            self.note(format!("release {:?}", request.paths));
            Ok(ReleaseReply {
                released: vec!["a.txt".into()],
                not_held: vec!["b.txt".into()],
                outside: vec!["/etc/passwd".into()],
            })
        }

        fn intent(
            &self,
            _caller: &Caller,
            request: &IntentRequest,
        ) -> Result<IntentReply, BackendError> {
            self.gate()?;
            self.note(format!("intent {}", request.text));
            Ok(IntentReply {
                intent: request.text.trim().to_owned(),
            })
        }

        fn why(&self, _caller: &Caller) -> Result<WhyReply, BackendError> {
            self.gate()?;
            Ok(WhyReply {
                decision: Some(DecisionInfo {
                    outcome: "deny".into(),
                    reason_code: "delete_outside_claims".into(),
                    message: "Claim `x.txt` before deleting it.".into(),
                    at: "2026-01-01T00:00:00Z".into(),
                }),
            })
        }

        fn lore_propose(
            &self,
            _caller: &Caller,
            request: &LoreProposeRequest,
        ) -> Result<LoreProposeReply, BackendError> {
            self.gate()?;
            self.note(format!(
                "lore {:?} {} {:?} {:?}",
                request.scope, request.text, request.replaces, request.why
            ));
            Ok(LoreProposeReply {
                proposal_id: "prop_1".into(),
                scope: request.scope.clone(),
                replaces: request.replaces.clone(),
                reviewer: Some("sara@example.com".into()),
            })
        }
    }

    fn args(value: Value) -> Option<Map<String, Value>> {
        value.as_object().cloned()
    }

    fn run(fake: &Fake, name: &str, arguments: Value) -> ToolOutput {
        call(fake, &Caller::default(), name, args(arguments))
    }

    #[test]
    fn six_tools_are_defined_with_schemas() {
        let defs = definitions();
        let listed: Vec<&str> = defs.iter().map(|d| d.name).collect();
        assert_eq!(
            listed,
            [
                "acs_status",
                "acs_claim",
                "acs_release",
                "acs_intent",
                "acs_why",
                "acs_lore_propose"
            ]
        );
        for def in &defs {
            assert_eq!(
                def.input_schema.get("type"),
                Some(&json!("object")),
                "{}",
                def.name
            );
            assert!(!def.input_schema.contains_key("$schema"));
            assert!(def.description.len() < 420, "{} is too long", def.name);
        }
        let claim = &defs[1];
        assert_eq!(
            claim.input_schema["properties"]["paths"]["type"],
            json!("array")
        );
        assert_eq!(claim.input_schema["required"], json!(["paths"]));
    }

    #[test]
    fn lore_propose_passes_everything_and_says_it_waits_for_review() {
        let fake = Fake::default();
        let out = run(
            &fake,
            LORE_PROPOSE,
            json!({"scope": "src/**", "text": "Use X.", "why": "saw it"}),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(
            out.text
                .contains("Proposed new lore for `src/**` as prop_1"),
            "{}",
            out.text
        );
        assert!(out.text.contains("sara@example.com"), "{}", out.text);
        assert!(
            out.text.contains("not shown to anyone until accepted"),
            "{}",
            out.text
        );
        let calls = fake.calls.lock().unwrap().clone();
        assert_eq!(calls, [r#"lore ["src/**"] Use X. None Some("saw it")"#]);

        let out = run(
            &fake,
            LORE_PROPOSE,
            json!({"scope": ["a/**"], "text": "  "}),
        );
        assert!(out.is_error);
        assert!(out.text.contains("empty"), "{}", out.text);
        let out = run(&fake, LORE_PROPOSE, json!({"scope": ["a/**"]}));
        assert!(out.is_error);
        assert!(out.text.contains("Pass `text`"), "{}", out.text);
        let out = run(
            &fake,
            LORE_PROPOSE,
            json!({"text": "Dollars.", "replaces": "lore_7f3a"}),
        );
        assert!(
            out.text.contains("a correction of `lore_7f3a`"),
            "{}",
            out.text
        );
    }

    #[test]
    fn claim_shows_the_lore_that_came_with_it() {
        let out = run(&Fake::default(), CLAIM, json!({"paths": ["src/a.rs"]}));
        assert!(
            out.text.contains(
                "

ACS lore for `src/**`"
            ),
            "{}",
            out.text
        );
    }

    #[test]
    fn claim_description_warns_about_deletes_and_recommends_claiming_up_front() {
        let defs = definitions();
        let claim = defs.iter().find(|d| d.name == CLAIM).map(|d| d.description);
        let text = claim.unwrap_or_default().to_lowercase();
        assert!(text.contains("delete"), "{text}");
        assert!(text.contains("denied"), "{text}");
        assert!(text.contains("claim a file before you delete"), "{text}");
        assert!(text.contains("up front"), "{text}");
    }

    #[test]
    fn status_text_lists_claims_sealed_paths_and_notes() {
        let out = run(&Fake::default(), STATUS, json!({}));
        assert!(!out.is_error);
        assert!(out.text.contains("branch `feature`"), "{}", out.text);
        assert!(
            out.text.contains("Your claims (1): `src/a.rs`"),
            "{}",
            out.text
        );
        assert!(
            out.text
                .contains("`src/b.rs` (omar@example.com, antigravity)"),
            "{}",
            out.text
        );
        assert!(out.text.contains("Never writable: `.env`"), "{}", out.text);
        assert!(
            out.text.contains("not set (call acs_intent)"),
            "{}",
            out.text
        );
        assert_eq!(
            out.structured
                .as_ref()
                .and_then(|v| v["target_branch"].as_str()),
            Some("main")
        );
    }

    #[test]
    fn status_without_a_session_still_answers_and_says_so() {
        let fake = Fake {
            no_session: true,
            ..Fake::default()
        };
        let out = run(&fake, STATUS, json!({}));
        assert!(!out.is_error);
        assert!(
            out.text.contains("no active ACS session yet"),
            "{}",
            out.text
        );
    }

    #[test]
    fn claim_passes_paths_and_reports_the_grant() {
        let fake = Fake::default();
        let out = run(&fake, CLAIM, json!({"paths": ["a.txt", "src"]}));
        assert!(!out.is_error);
        assert!(
            out.text.starts_with("Claimed 2 files: `a.txt`, `src`."),
            "{}",
            out.text
        );
        assert!(out.text.contains("Warning: the remote view is stale"));
        assert_eq!(
            fake.calls.lock().map(|c| c.clone()).unwrap_or_default(),
            vec![r#"claim ["a.txt", "src"]"#.to_owned()]
        );
        assert_eq!(
            out.structured.as_ref().and_then(|v| v["outcome"].as_str()),
            Some("granted")
        );
    }

    #[test]
    fn a_single_path_string_is_accepted() {
        let fake = Fake::default();
        let out = run(&fake, CLAIM, json!({"paths": "a.txt"}));
        assert!(!out.is_error, "{}", out.text);
        assert!(
            out.text.starts_with("Claimed 1 file: `a.txt`."),
            "{}",
            out.text
        );
    }

    #[test]
    fn a_refused_claim_is_an_error_result_with_the_gate_message() {
        let fake = Fake {
            refuse: true,
            ..Fake::default()
        };
        let out = run(&fake, CLAIM, json!({"paths": ["src/b.rs"]}));
        assert!(out.is_error);
        assert!(
            out.text.contains("Nothing was claimed (claim_conflict)"),
            "{}",
            out.text
        );
        assert!(
            out.text.contains("claimed by omar@example.com"),
            "{}",
            out.text
        );
    }

    #[test]
    fn bad_arguments_are_guidance_not_protocol_errors() {
        let fake = Fake::default();
        for (name, arguments) in [
            (CLAIM, json!({})),
            (CLAIM, json!({"paths": 3})),
            (CLAIM, json!({"paths": []})),
            (CLAIM, json!({"paths": [" "]})),
            (RELEASE, json!({"files": ["a"]})),
            (INTENT, json!({"text": "  "})),
            (INTENT, json!({})),
        ] {
            let out = run(&fake, name, arguments.clone());
            assert!(out.is_error, "{name} {arguments}");
            assert!(
                out.text.contains("Pass `"),
                "{name} {arguments}: {}",
                out.text
            );
        }
        assert!(
            fake.calls.lock().map(|c| c.is_empty()).unwrap_or(false),
            "no backend call for bad arguments"
        );
        let out = call(&fake, &Caller::default(), "acs_nope", None);
        assert!(out.is_error);
        assert!(out.text.contains("acs_claim"), "{}", out.text);
    }

    #[test]
    fn no_session_becomes_guidance() {
        let fake = Fake {
            no_session: true,
            ..Fake::default()
        };
        for (name, arguments) in [
            (CLAIM, json!({"paths": ["a"]})),
            (RELEASE, json!({"paths": ["a"]})),
            (INTENT, json!({"text": "x"})),
            (WHY, json!({})),
        ] {
            let out = run(&fake, name, arguments);
            assert!(out.is_error, "{name}");
            assert!(
                out.text.contains("first file edit will start one"),
                "{name}: {}",
                out.text
            );
        }
    }

    #[test]
    fn release_reports_all_three_groups() {
        let out = run(&Fake::default(), RELEASE, json!({"paths": ["a.txt"]}));
        assert!(!out.is_error);
        assert_eq!(
            out.text,
            "Released: `a.txt`. You held no claim on: `b.txt`. Outside the repository, ignored: `/etc/passwd`."
        );
    }

    #[test]
    fn intent_is_recorded_and_echoed() {
        let fake = Fake::default();
        let out = run(&fake, INTENT, json!({"text": " Refactor the parser "}));
        assert!(!out.is_error);
        assert!(
            out.text
                .contains("Intent recorded: \"Refactor the parser\""),
            "{}",
            out.text
        );
    }

    #[test]
    fn why_explains_the_last_decision() {
        let out = run(&Fake::default(), WHY, json!({}));
        assert!(!out.is_error);
        assert!(
            out.text.contains("denied") || out.text.contains("deny"),
            "{}",
            out.text
        );
        assert!(out.text.contains("delete_outside_claims"));
        assert!(out.text.contains("Claim `x.txt` before deleting it."));
    }

    #[test]
    fn why_with_no_decision_says_nothing_was_denied() {
        let text = why_text(&WhyReply::default());
        assert!(text.contains("no decision"), "{text}");
    }

    #[test]
    fn long_lists_are_cut() {
        let items: Vec<String> = (0..20).map(|i| format!("f{i}")).collect();
        let text = ticked(&items, LIST_LIMIT);
        assert!(text.ends_with("and 8 more"), "{text}");
    }
}
