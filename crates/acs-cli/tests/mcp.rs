//! End-to-end tests of `acs mcp`: the built binary is spawned and spoken to over stdio the way a
//! harness does (newline-delimited JSON-RPC), against a temporary repository and `ACS_HOME`.
//! Seats are created with `acs hook --canonical`, as a harness's hooks would.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use common::{Agent, World, acs_bin};
use serde_json::{Value, json};

/// A running `acs mcp` and a client for it.
struct Server {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    next_id: u64,
}

impl Server {
    /// Spawns `acs mcp <args>` in the world's repository with the given extra environment.
    fn start(world: &World, args: &[&str], env: &[(&str, &str)]) -> Self {
        let mut command = Command::new(acs_bin());
        command
            .arg("mcp")
            .args(args)
            .current_dir(&world.repo)
            .env("ACS_HOME", &world.home)
            .env("GIT_CONFIG_GLOBAL", &world.git_config)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("CLAUDE_CODE_SESSION_ID")
            .env_remove("CLAUDE_PROJECT_DIR")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        for (key, value) in env {
            command.env(key, value);
        }
        let mut child = command.spawn().unwrap();
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            stdin,
            lines,
            next_id: 1,
        }
    }

    fn send(&mut self, message: &Value) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{message}").unwrap();
        stdin.flush().unwrap();
    }

    /// Sends a request and returns the whole response object.
    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let line = self
                .lines
                .recv_timeout(Duration::from_secs(20))
                .expect("the server answers within 20 s");
            let value: Value = serde_json::from_str(&line)
                .unwrap_or_else(|err| panic!("stdout carries only JSON-RPC ({err}): {line}"));
            if value.get("id") == Some(&json!(id)) {
                return value;
            }
        }
    }

    /// The MCP handshake; returns the `initialize` result.
    fn initialize(&mut self, client_name: &str) -> Value {
        let response = self.request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": client_name, "version": "0.0.1"},
            }),
        );
        self.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        response["result"].clone()
    }

    /// `tools/call`; returns the `result` object.
    fn call(&mut self, name: &str, arguments: Value) -> Value {
        self.call_with_meta(name, arguments, None)
    }

    fn call_with_meta(&mut self, name: &str, arguments: Value, meta: Option<Value>) -> Value {
        let mut params = json!({"name": name, "arguments": arguments});
        if let Some(meta) = meta {
            params["_meta"] = meta;
        }
        let response = self.request("tools/call", params);
        assert!(
            response.get("error").is_none(),
            "a tool call never fails at the protocol level: {response}"
        );
        response["result"].clone()
    }

    fn finish(mut self) {
        drop(self.stdin.take());
        let status = self.child.wait().unwrap();
        assert!(
            status.success(),
            "the server exits cleanly at EOF: {status:?}"
        );
    }
}

fn text(result: &Value) -> String {
    result["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

fn is_error(result: &Value) -> bool {
    result["isError"].as_bool().unwrap_or(false)
}

#[test]
fn initialize_and_list_tools() {
    let world = World::new();
    let mut server = Server::start(&world, &[], &[]);
    let init = server.initialize("mcp-inspector");
    assert_eq!(init["serverInfo"]["name"], "acs");
    assert!(init["capabilities"]["tools"].is_object(), "{init}");
    assert!(
        init["instructions"]
            .as_str()
            .unwrap_or_default()
            .contains("acs_claim"),
        "{init}"
    );

    let listed = server.request("tools/list", json!({}));
    let tools = listed["result"]["tools"].as_array().unwrap().clone();
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    assert_eq!(
        names,
        [
            "acs_status",
            "acs_claim",
            "acs_release",
            "acs_intent",
            "acs_why",
            "acs_lore_propose"
        ]
    );
    for tool in &tools {
        assert_eq!(tool["inputSchema"]["type"], "object", "{tool}");
        assert_eq!(tool["_meta"]["antigravity/alwaysLoad"], true, "{tool}");
        assert!(tool["description"].as_str().unwrap().len() < 420, "{tool}");
    }
    let claim = tools.iter().find(|t| t["name"] == "acs_claim").unwrap();
    let description = claim["description"].as_str().unwrap();
    assert!(description.contains("denied by default"), "{description}");
    assert!(
        description.contains("claim a file before you delete"),
        "{description}"
    );
    assert!(description.contains("up front"), "{description}");
    assert_eq!(claim["inputSchema"]["required"], json!(["paths"]));
    assert_eq!(claim["inputSchema"]["properties"]["paths"]["type"], "array");
    server.finish();
}

#[test]
fn without_a_session_status_answers_and_the_rest_explain() {
    let world = World::new();
    let mut server = Server::start(&world, &["--harness", "claude-code"], &[]);
    server.initialize("claude-code");

    let status = server.call("acs_status", json!({}));
    assert!(!is_error(&status), "{status}");
    assert!(
        status.get("structuredContent").is_none(),
        "text only by default: {status}"
    );
    assert!(
        text(&status).contains("No active ACS session for claude-code yet"),
        "{}",
        text(&status)
    );
    assert!(
        text(&status).contains("Never writable"),
        "{}",
        text(&status)
    );

    for (tool, args) in [
        ("acs_claim", json!({"paths": ["a.txt"]})),
        ("acs_release", json!({"paths": ["a.txt"]})),
        ("acs_intent", json!({"text": "x"})),
        ("acs_why", json!({})),
    ] {
        let result = server.call(tool, args);
        assert!(is_error(&result), "{tool}: {result}");
        assert!(
            text(&result).contains("No active ACS session"),
            "{tool}: {}",
            text(&result)
        );
    }
    server.finish();
}

#[test]
fn claude_code_session_tools_end_to_end() {
    let world = World::new();
    let sara = Agent::new(&world, "claude-code", "cc-sara");
    let omar = Agent::new(&world, "antigravity", "ag-omar");
    sara.start();
    omar.start();

    let mut server = Server::start(
        &world,
        &["--harness", "claude-code", "--structured-content"],
        &[("CLAUDE_CODE_SESSION_ID", "cc-sara")],
    );
    server.initialize("claude-code");
    let meta = json!({"claudecode/toolUseId": "toolu_1"});

    // Status: a seat, no claims, the built-in sealed paths.
    let status = server.call_with_meta("acs_status", json!({}), Some(meta.clone()));
    let status_text = text(&status);
    assert!(!is_error(&status), "{status_text}");
    assert!(
        status_text.contains("You: claude-code session ses_"),
        "{status_text}"
    );
    assert!(status_text.contains("Your claims: none."), "{status_text}");
    assert!(status_text.contains("Never writable"), "{status_text}");
    assert_eq!(
        status["structuredContent"]["seat"]["harness"],
        "claude-code"
    );
    assert_eq!(status["structuredContent"]["target_branch"], "main");

    // Intent shows up in status.
    let intent = server.call("acs_intent", json!({"text": "  Rename util to helpers  "}));
    assert!(!is_error(&intent), "{}", text(&intent));
    assert!(
        text(&intent).contains("\"Rename util to helpers\""),
        "{}",
        text(&intent)
    );
    let status = server.call("acs_status", json!({}));
    assert!(
        text(&status).contains("Intent: \"Rename util to helpers\""),
        "{}",
        text(&status)
    );

    // Another agent holds src/lib.rs: our claim is refused with the holder named, and nothing
    // of the request (a.txt) is claimed.
    assert_eq!(
        omar.pre_edit("src/lib.rs").outcome,
        acs_core::Outcome::Allow
    );
    let refused = server.call("acs_claim", json!({"paths": ["a.txt", "src/lib.rs"]}));
    assert!(is_error(&refused), "{refused}");
    assert!(
        text(&refused).starts_with("Nothing was claimed (claim_conflict)."),
        "{}",
        text(&refused)
    );
    assert!(text(&refused).contains("src/lib.rs"), "{}", text(&refused));
    assert!(text(&refused).contains("antigravity"), "{}", text(&refused));
    assert_eq!(refused["structuredContent"]["outcome"], "refused");
    assert!(sara.claimed().is_empty());

    // Status now lists the other agent's claim.
    let status = server.call("acs_status", json!({}));
    assert!(text(&status).contains("`src/lib.rs`"), "{}", text(&status));
    assert!(text(&status).contains("antigravity"), "{}", text(&status));

    // A granted claim (a path, a directory, and a path outside the repository is refused whole).
    let granted = server.call("acs_claim", json!({"paths": ["a.txt", "src/util.rs"]}));
    assert!(!is_error(&granted), "{}", text(&granted));
    assert!(
        text(&granted).starts_with("Claimed 2 files:"),
        "{}",
        text(&granted)
    );
    assert_eq!(sara.claimed(), ["a.txt", "src/util.rs"]);
    // Claiming again is idempotent.
    let again = server.call("acs_claim", json!({"paths": ["a.txt"]}));
    assert!(!is_error(&again), "{}", text(&again));
    assert_eq!(sara.claimed(), ["a.txt", "src/util.rs"]);

    // The hook path agrees: the claimed file can be deleted, an unclaimed one cannot, and
    // acs_why explains the denial.
    assert_eq!(
        sara.pre_tool("Bash", "delete", &["src/util.rs"]).outcome,
        acs_core::Outcome::Allow
    );
    let denied = sara.pre_tool("Bash", "delete", &["src/lib.rs"]);
    assert_eq!(denied.outcome, acs_core::Outcome::Deny);
    let why = server.call("acs_why", json!({}));
    assert!(!is_error(&why), "{}", text(&why));
    assert!(text(&why).contains("deny"), "{}", text(&why));
    assert!(
        text(&why).contains("delete_outside_claims"),
        "{}",
        text(&why)
    );
    assert!(
        text(&why).contains(&denied.message),
        "{} / {}",
        text(&why),
        denied.message
    );
    assert_eq!(
        why["structuredContent"]["decision"]["reason_code"],
        "delete_outside_claims"
    );

    // The harness's own pre-tool hook for the next MCP call is an uneventful allow; it does not
    // replace the denial, so acs_why still gives the gate's original wording.
    let hooked = sara.pre_tool("mcp__acs__acs_why", "other", &[]);
    assert_eq!(hooked.outcome, acs_core::Outcome::Allow);
    let why = server.call("acs_why", json!({}));
    assert!(
        text(&why).contains("delete_outside_claims"),
        "{}",
        text(&why)
    );
    assert!(text(&why).contains(&denied.message), "{}", text(&why));

    // Release: one held, one not held.
    let released = server.call("acs_release", json!({"paths": ["a.txt", "b.txt"]}));
    assert!(!is_error(&released), "{}", text(&released));
    assert!(
        text(&released).contains("Released: `a.txt`"),
        "{}",
        text(&released)
    );
    assert!(
        text(&released).contains("held no claim on: `b.txt`"),
        "{}",
        text(&released)
    );
    assert_eq!(sara.claimed(), ["src/util.rs"]);

    // Bad arguments and unknown tools are results with guidance, not protocol errors.
    let bad = server.call("acs_claim", json!({}));
    assert!(is_error(&bad), "{bad}");
    assert!(text(&bad).contains("Pass `paths`"), "{}", text(&bad));
    let unknown = server.call("acs_teleport", json!({}));
    assert!(is_error(&unknown), "{unknown}");
    assert!(text(&unknown).contains("acs_claim"), "{}", text(&unknown));
    server.finish();
}

#[test]
fn releasing_a_directory_releases_the_claims_under_it() {
    let world = World::new();
    let sara = Agent::new(&world, "claude-code", "cc-sara");
    sara.start();
    let mut server = Server::start(
        &world,
        &["--harness", "claude-code"],
        &[("CLAUDE_CODE_SESSION_ID", "cc-sara")],
    );
    server.initialize("claude-code");

    let granted = server.call("acs_claim", json!({"paths": ["src", "a.txt"]}));
    assert!(!is_error(&granted), "{}", text(&granted));
    assert_eq!(sara.claimed(), ["a.txt", "src/lib.rs", "src/util.rs"]);

    // A directory releases every claim under it, and the reply names the files.
    let released = server.call("acs_release", json!({"paths": ["src/"]}));
    assert!(!is_error(&released), "{}", text(&released));
    assert!(
        text(&released).contains("Released: `src/lib.rs`, `src/util.rs`"),
        "{}",
        text(&released)
    );
    assert_eq!(sara.claimed(), ["a.txt"]);

    // The repository root covers everything the seat holds.
    server.call("acs_claim", json!({"paths": ["src"]}));
    let released = server.call("acs_release", json!({"paths": ["."]}));
    assert!(!is_error(&released), "{}", text(&released));
    assert!(
        text(&released).contains("Released: `a.txt`, `src/lib.rs`, `src/util.rs`"),
        "{}",
        text(&released)
    );
    assert!(sara.claimed().is_empty());
    server.finish();
}

#[test]
fn claims_bring_lore_and_agents_propose_lore_for_review() {
    let world = World::new();
    world.write(
        ".acs/policy/lore/src/errors.md",
        "+++\nid = \"lore_err\"\nscope = [\"src/**\"]\nowner = \"lina@example.com\"\n\
         written_against = \"HEAD\"\n+++\nErrors use thiserror.\n",
    );
    world.write(
        ".acs/policy/territory.toml",
        "[[zone]]\npaths = [\"src/**\"]\nkind = \"shared\"\nowners = [\"lina@example.com\"]\n",
    );
    let sara = Agent::new(&world, "claude-code", "cc-sara");
    sara.start();
    let mut server = Server::start(
        &world,
        &["--harness", "claude-code"],
        &[("CLAUDE_CODE_SESSION_ID", "cc-sara")],
    );
    server.initialize("claude-code");

    // The claim's reply carries the lore, once.
    let claimed = server.call("acs_claim", json!({"paths": ["src/lib.rs"]}));
    assert!(!is_error(&claimed), "{}", text(&claimed));
    assert!(
        text(&claimed).contains("ACS lore for `src/**` (lore_err"),
        "{}",
        text(&claimed)
    );
    assert!(text(&claimed).contains("Errors use thiserror."));
    let again = server.call("acs_claim", json!({"paths": ["src/util.rs"]}));
    assert!(!text(&again).contains("ACS lore"), "{}", text(&again));
    // ... and the hook does not repeat it either.
    let edit = sara.pre_edit("src/util.rs");
    assert!(edit.context.iter().all(|c| !c.contains("lore_err")));

    // A proposal is stored for review, never written to the repository.
    let proposed = server.call(
        "acs_lore_propose",
        json!({"scope": ["./src/**"], "text": "Use anyhow only in main.", "why": "seen in main.rs"}),
    );
    assert!(!is_error(&proposed), "{}", text(&proposed));
    let reply = text(&proposed);
    assert!(
        reply.contains("Proposed new lore for `src/**` as prop_"),
        "{reply}"
    );
    assert!(reply.contains("Its owner (lina@example.com)"), "{reply}");
    assert!(
        !world
            .repo
            .join(".acs/policy/lore/src")
            .read_dir()
            .unwrap()
            .any(|e| { e.unwrap().file_name() != "errors.md" })
    );
    let store = acs_lore::ProposalStore::new(world.repo_home().pending_dir().join("lore"));
    let (pending, _) = store.list();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].scope, ["src/**"]);
    assert_eq!(pending[0].proposed_by.harness, "claude-code");
    assert_eq!(pending[0].why.as_deref(), Some("seen in main.rs"));

    // A correction names the entry; a bad one says what to fix.
    let fix = server.call(
        "acs_lore_propose",
        json!({"text": "Errors use thiserror 2.", "replaces": "lore_err"}),
    );
    assert!(
        text(&fix).contains("a correction of `lore_err` for `src/**`"),
        "{}",
        text(&fix)
    );
    let bad = server.call("acs_lore_propose", json!({"text": "Something."}));
    assert!(is_error(&bad));
    assert!(
        text(&bad).contains("Say where the lore applies"),
        "{}",
        text(&bad)
    );
    let bad = server.call(
        "acs_lore_propose",
        json!({"text": "x", "replaces": "lore_nope", "scope": "src/**"}),
    );
    assert!(
        text(&bad).contains("no lore entry `lore_nope`"),
        "{}",
        text(&bad)
    );
    assert_eq!(store.list().0.len(), 2);
    server.finish();
}

#[test]
fn a_stale_environment_id_falls_back_to_the_live_seat_of_the_harness() {
    let world = World::new();
    let only = Agent::new(&world, "claude-code", "cc-real");
    only.start();
    // Claude Code after `--continue`: the server's environment still has the startup id.
    let mut server = Server::start(&world, &[], &[("CLAUDE_CODE_SESSION_ID", "cc-startup-id")]);
    server.initialize("claude-code");
    let granted = server.call("acs_claim", json!({"paths": ["a.txt"]}));
    assert!(!is_error(&granted), "{}", text(&granted));
    assert_eq!(only.claimed(), ["a.txt"]);

    // A second live session makes the fallback a guess, and the agent is told so.
    let second = Agent::new(&world, "claude-code", "cc-second");
    second.start();
    let status = server.call("acs_status", json!({}));
    assert!(
        text(&status).contains("2 sessions are active"),
        "{}",
        text(&status)
    );
    // The most recently seen one wins; an exact id still binds exactly.
    server.finish();
    let mut exact = Server::start(&world, &[], &[("CLAUDE_CODE_SESSION_ID", "cc-real")]);
    exact.initialize("claude-code");
    let status = exact.call("acs_status", json!({}));
    assert!(
        text(&status).contains("Your claims (1): `a.txt`"),
        "{}",
        text(&status)
    );
    assert!(
        !text(&status).contains("sessions are active"),
        "{}",
        text(&status)
    );
    exact.finish();
}

#[test]
fn antigravity_binds_per_call_from_the_root_conversation_id() {
    let world = World::new();
    let a = Agent::new(&world, "antigravity", "conv-a");
    let b = Agent::new(&world, "antigravity", "conv-b");
    a.start();
    b.start();
    let mut server = Server::start(&world, &["--structured-content"], &[]);
    server.initialize("antigravity-client");
    let key = "antigravity.google/root_conversation_id";

    let claim_a = server.call_with_meta(
        "acs_claim",
        json!({"paths": ["a.txt"]}),
        Some(json!({ key: "conv-a" })),
    );
    assert!(!is_error(&claim_a), "{}", text(&claim_a));
    let claim_b = server.call_with_meta(
        "acs_claim",
        json!({"paths": ["src/lib.rs"]}),
        Some(json!({ key: "conv-b" })),
    );
    assert!(!is_error(&claim_b), "{}", text(&claim_b));
    assert_eq!(a.claimed(), ["a.txt"]);
    assert_eq!(b.claimed(), ["src/lib.rs"]);

    // B cannot take A's file, and is told who has it.
    let clash = server.call_with_meta(
        "acs_claim",
        json!({"paths": ["a.txt"]}),
        Some(json!({ key: "conv-b" })),
    );
    assert!(is_error(&clash), "{}", text(&clash));
    assert!(text(&clash).contains("a.txt"), "{}", text(&clash));

    server.call_with_meta(
        "acs_intent",
        json!({"text": "tests"}),
        Some(json!({ key: "conv-a" })),
    );
    let status = server.call_with_meta("acs_status", json!({}), Some(json!({ key: "conv-b" })));
    assert!(
        text(&status).contains("Your claims (1): `src/lib.rs`"),
        "{}",
        text(&status)
    );
    assert!(text(&status).contains("`a.txt`"), "{}", text(&status));
    assert!(
        !text(&status).contains("2 sessions are active"),
        "{}",
        text(&status)
    );
    assert_eq!(
        status["structuredContent"]["seat"]["session_id"].as_str(),
        Some(b.info().session_id.as_str())
    );
    server.finish();
}

#[test]
fn replays_the_requests_captured_from_claude_code() {
    // Claude Code 2.1.314 speaks the stateless protocol revision: a `server/discover` probe,
    // no `initialize`, and its identity in each request's `_meta`.
    let captured = include_str!("../../../tests/fixtures/claude-code/mcp_requests.captured.jsonl");
    let world = World::new();
    Agent::new(&world, "claude-code", "cc-fixture").start();
    let mut server = Server::start(
        &world,
        &["--harness", "claude-code"],
        &[("CLAUDE_CODE_SESSION_ID", "cc-fixture")],
    );
    let mut responses = std::collections::HashMap::new();
    for line in captured.lines() {
        let request: Value = serde_json::from_str(line).unwrap();
        server.send(&request);
        let response = loop {
            let line = server
                .lines
                .recv_timeout(Duration::from_secs(20))
                .expect("the server answers within 20 s");
            let value: Value = serde_json::from_str(&line).unwrap();
            if value["id"] == request["id"] {
                break value;
            }
        };
        responses.insert(request["id"].to_string(), response);
    }
    let discover = &responses["\"server-discover-probe-1\""]["result"];
    assert!(
        discover["supportedVersions"]
            .as_array()
            .is_some_and(|v| v.contains(&json!("2026-07-28"))),
        "{discover}"
    );
    assert_eq!(discover["capabilities"]["tools"], json!({}));
    let tools = &responses["0"]["result"]["tools"];
    assert_eq!(tools.as_array().map(Vec::len), Some(6), "{tools}");
    let intent = &responses["1"]["result"];
    assert!(
        text(intent).contains("Intent recorded: \"capture check\""),
        "{intent}"
    );
    let status = &responses["2"]["result"];
    assert!(
        text(status).contains("You: claude-code session ses_"),
        "{status}"
    );
    assert!(
        text(status).contains("Intent: \"capture check\"."),
        "{status}"
    );
    server.finish();
}
