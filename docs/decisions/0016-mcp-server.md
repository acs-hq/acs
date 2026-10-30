# 0016 — MCP server (WP2.6)

Date: 2026-10-30. Status: accepted (WP2.6), pending orchestrator review.

## Context

WP2.6 adds `acs mcp`: five tools an agent can call (`acs_status`, `acs_claim`, `acs_release`,
`acs_intent`, `acs_why`). Decision 0011 fixed the architecture: `acs-mcp` is backend-agnostic and
defines a `Backend` trait, `acs-cli` implements it with the `runtime` operations, and the server
binds to a seat in a fixed order (section 5). This record lists the choices made beyond that, the
protocol shapes relied on, and what was checked against a real harness.

## Decision

### Crates and the SDK

- **`rmcp` 3.8.0** (Apache-2.0, MSRV 1.88; released 2026-10-26, the latest on crates.io when
  chosen) with `default-features = false, features = ["server", "transport-io"]` (no macros, no
  HTTP, no auth). It pulls tokio, futures, chrono (through its schemars feature) and tracing; all
  licenses are MIT or Apache-2.0 (checked from `cargo metadata`; `cargo-deny` itself is not
  installed here). `tokio` is a direct dependency of `acs-mcp` with `default-features = false,
  features = ["rt"]` to build the runtime; both go into root `[workspace.dependencies]`.
- The API differs from what older write-ups show: `ServerConfig` (= `InitializeResult`),
  `CallToolResponse`, `RequestContext::meta`, `#[non_exhaustive]` model structs built with
  constructors. The handler implements `ServerHandler` by hand (`get_info`, `initialize`,
  `list_tools`, `call_tool`) rather than with the `#[tool]` macros, so tool definitions are plain
  data (`tools::definitions`) that unit tests can read and `_meta` can be set per tool.
- **No async runtime on the hook path.** Only `acs_mcp::serve_stdio` builds a runtime (current
  thread), and only `acs mcp` calls it. `acs hook` links tokio but never initializes it.
- **Modules.** `backend` (trait, typed serializable requests and replies, `Caller`),
  `bind` (what the protocol layer learns about the harness session), `tools` (definitions,
  dispatch, result text; no dependency on rmcp), `server` (the handler). Backend methods are
  synchronous: runtime operations are, and `RepoContext` is not `Send`. The server handles one
  request at a time on one thread, so a blocking call starves nothing.
- Timestamps in replies are RFC 3339 strings to a precision of seconds, so `acs-mcp` depends on
  nothing but serde, schemars, rmcp, tokio and thiserror.

### Tools

- Descriptions are one to four sentences, concrete, under 420 characters (asserted). `acs_claim`
  says that deletes and moves outside one's own claims are denied by default, to claim a file
  before deleting it, to claim every file of a large change up front, and that a directory claims
  the files under it. Schemas come from `schemars` on the argument structs, with `$schema`,
  `title` and the struct-level `description` removed (doc comments on argument structs would
  otherwise reach the model); `properties` is always present.
- Every tool carries `_meta: {"antigravity/alwaysLoad": true}` (Antigravity otherwise loads MCP
  tools on demand through `call_mcp_tool`) and annotations (`readOnlyHint` for status and why,
  `idempotentHint` for all, `destructiveHint: false`, `openWorldHint: false`).
- `paths` accepts a list or a single string (agents send both). Relative paths are relative to
  the **repository root** (not the server's directory), so "src/a.rs" means the same thing to the
  agent and the gate. A directory claims the files under it (`runtime::claim_paths` expands it).
- **Errors are results, not protocol errors**: bad arguments, an unknown tool name, no session
  yet, an unavailable state database and a refused claim are `isError: true` results whose text
  says what to do. A refused claim is an error result because nothing happened; the text is
  `Nothing was claimed (<reason_code>). <the gate's own message>`.
- **Results are text, and `structuredContent` is off by default** (`acs mcp --structured-content`
  turns it on). Found with the real client: Claude Code 2.1.314 shows the model the structured
  JSON *instead of* the text content when both are present (observed in `claude -p
  --output-format stream-json`: the `tool_result` was the JSON). The text is what is written for
  the agent, so it must be what the agent sees. The structured form is still produced (the
  backend's reply, serialized) for clients that want it.
- `acs_status` answers without a session (it shows claims, sealed paths and a note), the other
  four say `No active ACS session for <harness> yet. ACS starts one when your first file edit or
  shell command runs; try again after that.` Lists are cut at 12 items ("and N more").
- `acs_intent` stores through `runtime::set_intent` (trimmed, 500 characters) and echoes what was
  stored.

### Protocol shapes relied on

- **Claude Code 2.1.314 speaks the stateless draft revision `2026-07-28`**, not the classic
  handshake: it sends a `server/discover` probe (`id: "server-discover-probe-1"`), then
  `tools/list`, then `tools/call`, with no `initialize`. Every request's `params._meta` carries
  `io.modelcontextprotocol/protocolVersion`, `.../clientInfo` (`{name: "claude-code", title,
  version: "2.1.314", ...}`), `.../clientCapabilities`, and on `tools/call` also
  `claudecode/toolUseId` and `progressToken`. rmcp 3.8.0 answers both this and the classic
  handshake (checked: the `claude -p` run below, and the tests use `initialize`). Because the
  client identity may arrive only in `_meta`, the harness is also inferred from
  `_meta["io.modelcontextprotocol/clientInfo"].name` when there was no `initialize`.
  Captured requests: `tests/fixtures/claude-code/mcp_requests.captured.jsonl` (replayed by a test).
- **Claude Code** also puts `CLAUDE_CODE_SESSION_ID` and `CLAUDE_PROJECT_DIR` in the server's
  environment (checked in the same run: the value equals the `--session-id` given to `claude`).
  Plugin-provided servers are named `mcp__plugin_<plugin>_<server>__<tool>`, a `--mcp-config`
  server `mcp__<server>__<tool>`; tool names do not matter to the server.
- **Antigravity** (from `docs/adapters/antigravity.md` on `wp2.3-antigravity`, not run here):
  every `tools/call` carries `params._meta["antigravity.google/root_conversation_id"]` since
  1.4.4, with the top-level conversation id also for subagents' calls; nothing at `initialize` or
  in the environment. A documented-shape fixture is `tests/fixtures/antigravity/
  mcp_tools_call.documented.json` (documented, not captured).
- Transport: newline-delimited JSON-RPC on stdin and stdout; nothing else writes to stdout.

### Seat binding

The protocol layer (`acs-mcp::bind`) turns each call into a `Caller { harness, session_id,
tool_use_id }`; the backend (`acs-cli`) resolves it, per call, in this order (decision 0011 §5):

1. **Harness**: `--harness`; else Antigravity if its `_meta` key is present; else Claude Code if
   the tool use id is present or `CLAUDE_CODE_SESSION_ID` is set; else the client name
   (`claude*` or `antigravity*`); else unknown (then any agent harness matches).
2. **Session id** from the source of *that* harness only: Claude Code's environment variable;
   Antigravity's per-call `_meta`. Used only if it names a **live** seat of that harness (the
   variable is stale after `--continue` and `/clear`).
3. Else the most recently seen live seat of the harness whose work tree is the server's work
   tree. If several qualify, the agent is told so ("N sessions are active ... used the most
   recently active one"), as a warning on `acs_claim` and a note on `acs_status`.
4. Else no seat (see above). `human` seats are never bound.

The server's work tree is the repository around its current directory, else around
`CLAUDE_PROJECT_DIR`. A seat in another work tree of the same repository (Claude Code's
`EnterWorktree`) is only found by an exact session id match... and only if the repository around
the server is the same one; see Consequences.

`_meta["claudecode/toolUseId"]` is extracted into `Caller::tool_use_id` but **not used**: binding
by it needs the hook pipeline to persist `(tool_use_id -> seat)` for MCP pre-tool events, which
WP2.1 does not do. It would fix the one remaining ambiguity (two Claude Code sessions in one work
tree after `--continue`).

### `acs_why` and the hook overwriting the last decision

`runtime::last_decision` is the last pre-tool decision of the seat, and `hook::pre::record`
overwrites it on **every** pre-tool event, including the harness's own pre-tool hook for the
`acs_why` call, which is an uneventful allow (`ok`, empty message). Read naively, `acs_why` would
always answer "allowed". Workaround, inside this WP: when the stored decision is an allow with an
empty message, `acs_why` looks through the seat's event log for the most recent denied or asked
pre-tool event and describes it ("Earlier, `Bash` on `src/lib.rs` was blocked
(delete_outside_claims). The original wording was not kept: ..."), since events keep the reason and
the paths but not the message. A denial that is still the last decision is returned verbatim, with
the gate's message. See the contract change request in the report: the proper fix is in the
pipeline (do not overwrite a denial with a path-less allow).

### Checked with a real harness

`claude` 2.1.314, `claude -p --session-id <uuid> --strict-mcp-config --mcp-config <json>` in a
throwaway repository with `ACS_HOME` in the scratchpad and the built `acs` first on `PATH` for
that run only (no global or project settings touched). A seat was created first with `acs hook
--canonical session_start` for the same session id. Claude Code listed the server (`status:
connected`, five tools `mcp__acs__acs_*`), and Claude called the tools. With another seat
(`antigravity`) holding `src/lib.rs`, the results the model saw were:

```
acs_intent {"text":"update the library entry point"}
  Intent recorded: "update the library entry point". Other agents and the user see it in acs_status.
acs_claim {"paths":["src/lib.rs"]}   (isError: true)
  Nothing was claimed (claim_conflict). `src/lib.rs` is claimed by t@example.com (antigravity,
  active just now). Work on other files, or ask your pilot to coordinate.
acs_why {}
  Your last action was deny (claim_conflict, 2026-10-30T21:15:07Z).
  `src/lib.rs` is claimed by t@example.com (antigravity, active just now). Work on other files, ...
acs_status {}
  Repository <repo>, branch `main`, merging into `main`.
  You: claude-code session ses_..., pilot t@example.com. Intent: "update the library entry point".
  Your claims: none.
  Claimed by others (1):
  - `src/lib.rs` (t@example.com, antigravity)
  Never writable: `.git/**`, `.acs/**`, `.github/workflows/**`, `**/.env*`, ...
```

and the model's own summary named the holder correctly. **Antigravity could not be run here**; its
binding rests on its documented `_meta` key (unit-tested with the documented shape, and by an
end-to-end test that sends the key) and its `alwaysLoad` marker is documented (changelog 1.3.3),
neither verified against the real client. The MCP inspector needs a browser; the real-client run
and the stdio tests (`initialize`, `tools/list`, every tool) stand in for it.

## Consequences

- `acs mcp` works without a session (status only) and starts nothing itself: a seat exists only
  after a hook event. With no hooks installed (as in the `claude -p` check) the tools bind only to
  seats created by hand.
- Two Claude Code sessions in one work tree, one of them resumed with `--continue` or after
  `/clear`, can be confused; the agent is warned, and the tool use id could remove the ambiguity
  once the pipeline persists it. An exact environment id match is never ambiguous.
- Paths are resolved against the repository around the server. An agent that has entered another
  git work tree of the same repository after startup still claims in the server's work tree.
- Claims made through `acs_claim` carry the config TTL and are refreshed by the same post-tool
  heartbeats as edits; `acs_release` and the session end release them.
- With `--structured-content` the structured JSON replaces the text in Claude Code; leave it off
  for agents.
- Windows and macOS were not run. `acs-mcp` type-checks for `x86_64-pc-windows-gnu` with
  clippy; `acs-cli` has C dependencies (bundled SQLite) and was not cross-checked.
