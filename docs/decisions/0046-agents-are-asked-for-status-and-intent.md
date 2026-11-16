# 0046 — ACS asks every session for `acs_status` and `acs_intent`, once

Date: 2026-11-16. Status: accepted.

Decided by: the human that an agent must make both calls whatever its task is, in every
repository that uses ACS, and that Antigravity must not call `acs_status` with every message;
Claude for how (the human said the issue's own options need not be followed).

## Context

Issue 73: a Claude Code session edited a dozen files in this repository and never called
`acs_status`, `acs_intent` or `acs_release`. Claims happened only because the edit hook makes
them. Its session record says "Intent: none given", and so would `acs log`, `acs blame` and
what another agent is told about the holder of a file. The session-start message began "Editing
a file claims it for you", which reads as "ACS is automatic, nothing to do"; the two calls were
asked for only in the MCP server's instructions, among the other tools, and in a skill the
agent had to choose to load.

Separately, the human saw the opposite in the Antigravity desktop app: the agent called
`acs_status` after every message. The plugin's rule said "At the start of every task, call
`acs_status`", and each message is a task.

## Decision

1. **The session-start message leads with the two calls.** `StatusView::summary_lines`
   (`crates/acs-cli/src/runtime/ops.rs`) now says, after one sentence on what ACS is: "Before
   you start on the user's request, whatever it is, make these two tool calls. Once per session
   is enough", then `acs_status` and `acs_intent` as a numbered list, then the tools' names in
   that harness (`tool_names`: `mcp__plugin_acs_acs__...` with the ToolSearch query for Claude
   Code, `mcp_acs_acs_acs_...` for Antigravity; nothing for a harness ACS ships no plugin for).
   "Editing a file claims it for you" comes after, with "free them with `acs_release`".
   The text is written by `acs`, so it reaches every repository with the next install and
   needs nothing in a repository's own instruction files.
2. **A session that already gave its intent is not asked again.** A resume, a compaction or a
   harness that reports a start more than once gets "Your intent on record: ..." instead.
3. **One reminder for a session that skipped it** (`crates/acs-cli/src/hook/intent.rs`). The
   first edit, write, delete, move or shell command of a session with no intent on record
   carries "this session is changing the repository and has not said what it is doing. Call
   `acs_intent` now". Claude Code gets it in that call's context. Antigravity shows no context
   with an allowed tool call, so there it waits in a state flag
   (`intent_reminder:<seat>`, value `due`) and goes out with the next `PreInvocation`. Once per
   session; a denied call does not use it up; session end clears the flag. It is not a notice
   (decision 0036): the engine marks notices delivered at pre-tool, which would lose them in
   Antigravity.
4. **Nothing is blocked.** An agent that ignores both messages still works. Refusing edits
   until an intent exists was not built: where the MCP server is missing (hooks only), no agent
   could ever comply.
5. **The MCP instructions and tool descriptions say "once per session, before other work"**
   (`acs_mcp::tools`), and no longer "at the start of a task". The instructions grew from under
   420 to under 520 characters.
6. **The Antigravity rule says once per conversation** and, in bold, not to call `acs_status`
   again with each later message; `acs_intent` again only for a different task. The usage skill
   (three exact copies) says the same.
7. **Issue 73's option 2 (a line in this repository's `CLAUDE.md`) was not done.** The human
   asked for a fix that holds in every repository; this one needs no such line.

## Evidence

`tests/e2e/real/first_calls.sh` (new), run 2026-11-16 on Windows with tasks that never mention
ACS:

- Claude Code 2.1.316, Sonnet, three separate sessions (add a function, answer a question
  that needs no edit, create a test file): each called `acs_status` once and `acs_intent`
  before its first change, and its record carries the intent. The two sessions that changed
  files also called `acs_release` at the end.
- `agy` 1.5.9, `gemini-3.8-flash-medium`, one conversation of three messages, run twice:
  `acs_status` once, with the first message; `acs_intent` three times (one per task);
  `acs_release` after each task. ACS's start message came once.

Tests: `session_start_asks_for_status_and_intent_until_an_intent_is_given`,
`a_session_that_changes_files_without_an_intent_is_reminded_once` and
`antigravity_gets_the_intent_reminder_with_its_next_invocation` in
`crates/acs-cli/tests/e2e.rs`.

## Not done, not checked

- **No run with the old text for comparison**, so the runs show that agents comply now, not
  how often they failed before in a small repository.
- **The reminder (item 3) never fired in a real session**: every real agent made the calls from
  the start message. Only the tests exercise it.
- **The desktop apps** (Claude Code's and Antigravity's) were not run; the human saw the
  repeated `acs_status` there, and only they can confirm it is gone. A repository that already
  has the Antigravity plugin keeps the old rule until `acs plugin install antigravity` runs
  again.
- **Permission prompts.** The real Claude Code sessions ran headless with the ACS tools
  allowed. In an interactive session the first `acs_status` and `acs_intent` may each ask the
  user for permission, unless the user allows the plugin's tools.
- A long session with a very large context (the case of issue 73) was not reproduced.
