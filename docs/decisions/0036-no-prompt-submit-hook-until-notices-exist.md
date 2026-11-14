# 0036 — The Claude Code plugin registers no prompt-submit hook until notices exist

Date: 2026-11-14. Status: accepted. Decided by: the human (remove the hook from Claude Code
now, and from Antigravity too if it is there); Claude for leaving Antigravity as it is, with the
reason below.

## Context

The Claude Code plugin ran `acs hook claude-code` every time the user sent a message
(`UserPromptSubmit`). That call has one job: handing an agent the notices waiting for it. A
notice is a short message from ACS to an agent, such as "the file you were waiting for is free
now" or "another agent changed a function your files use". The delivery was built in Phase 2;
what creates notices is Phase 8 (WP8.2 interface changes, WP8.3 queued edits), which is not
built. So the call started `acs` on every message and never had anything to deliver. Issue 70
asked whether it should stay registered.

## Decision

1. **`integrations/claude-code/hooks/hooks.json` no longer lists `UserPromptSubmit`.** Claude
   Code then starts no `acs` process when the user sends a message.
2. **Nothing else was removed.** The adapter still parses and renders the event, the hook
   pipeline still handles `prompt_submit`, and the fixtures stay, so putting it back is one
   block in `hooks.json`.
3. **Antigravity keeps its `PreInvocation` hook.** The human asked for it to go there too, but
   it is not the same thing: Antigravity has no session-start event, so the first
   `PreInvocation` of a conversation is what registers the session and gives the agent the
   ACS status (decision 0014, `acs-adapters`' `antigravity.rs`). Removing it would remove that
   message. Later `PreInvocation` calls end early with nothing to deliver; Antigravity's hook
   file cannot ask for the first call only.

## Consequences

- **Whoever builds the first notice must register the hook again**, in `hooks.json` and in the
  test `the_plugin_registers_exactly_the_events_the_adapter_parses`
  (`crates/acs-adapters/src/claude_code.rs`), which now expects five events and says why.
  Without it, Claude Code agents would get notices only with their next tool call's context,
  not with the next message.
- The change reaches a Claude Code user when their copy of the plugin updates, which needs this
  commit on GitHub. No `acs` reinstall is needed for it.
- One of issue 70's three open questions is answered. The issue stays open for the calls
  around reads.

## Not done

- No real Claude Code run. A hook that is not listed is not called; the plugin was not
  reloaded in a session to watch it.
