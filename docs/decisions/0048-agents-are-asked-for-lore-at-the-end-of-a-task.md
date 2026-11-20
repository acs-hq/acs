# 0048 — Agents are asked once, at the end of a task, to propose lore

Date: 2026-11-20. Status: accepted.

Decided by: the human to do issue 80's overall recommendation (A, C, and the documentation part
of D) with B as an extra; Claude for how.

## Context

Issue 80: since the lore phase was built, no agent working in this repository has called
`acs_lore_propose`. Agents were asked to propose only in texts they do not read when they finish
(the MCP instructions, the tool's description, one bullet of the usage skill). In Claude Code the
tool was never even loaded: ACS tools are loaded through ToolSearch, and the start message named
only `acs_status` and `acs_intent`. No hook fired at the end of a turn, and a proposal that did
exist waited without the pilot being told, except in the CLI's `acs status`.

## Decision

1. **A new canonical phase, `turn_end`** (`acs_core::Phase::TurnEnd`, SPEC section 3, schema
   regenerated): the agent is about to end its turn. Claude Code's and Codex's `Stop` and
   Antigravity's `Stop` map to it; `acs hook <harness> --phase turn_end` names it. Context in a
   `turn_end` decision is rendered as the harness's "keep going" answer
   (`{"decision":"block","reason":...}` in Claude Code and Codex,
   `{"decision":"continue","reason":...}` in Antigravity), and nothing else is: warnings and the
   message are dropped there (`acs_adapters::turn_end_request`), so a repository that cannot be
   opened never keeps an agent going. SPEC says an implementation returns context at `turn_end`
   at most once per seat.
2. **The plugins register `Stop`** (Claude Code `hooks.json`, the Codex `config.toml` block, the
   Antigravity `hooks.json`). Antigravity's `Stop` was deliberately not registered before
   (adapter doc 2.7): it was proposed as a session end, which it is not. It is a turn end here,
   and releases nothing.
3. **Option A: the request** (`crates/acs-cli/src/hook/lore_prompt.rs`). The first allowed
   edit, write, delete, move, or shell command that is expected to write marks the seat (flag
   `lore_prompt:<seat>`, `due`); a denied call does not. At the next `turn_end` the flag goes
   from `due` to shown in one statement (`StateDb::replace_flag`, new), and only the process
   that changed it returns the request. So it comes once per seat, after files changed, and a
   flag that cannot be read or written means no request: the agent is never asked twice and
   never kept in a loop. Session end clears the flag. Nothing happens where `lore` is off:
   `acs_lore_propose` is not listed there (decision 0034). The request says what lore is for,
   that the state of the work belongs in hand-off notes or issues, that a person reviews a
   proposal, and that ACS will not ask again; it names the tool in the agent's harness.
4. **"Never block" is read as "never trap".** Issue 80 asked for a hook that never blocks. In
   Claude Code and Codex a `Stop` hook reaches the model only by blocking the stop (its
   `additionalContext` would wait for the next user message, after the task is over). So ACS
   blocks exactly one stop per session, which costs the agent one more turn; every other `Stop`
   gets no answer. `stop_hook_active` is not read: the once-per-seat flag already covers it.
5. **Option B: the tool is loaded at the start.** Where lore is on, Claude Code's session-start
   message adds `mcp__plugin_acs_acs__acs_lore_propose` to the ToolSearch query it gives,
   saying ACS may ask for it at the end of a task. Antigravity and Codex list every MCP tool
   anyway; their start message is unchanged.
6. **Option C: the pilot is told.** The `acs_lore_propose` reply now asks the agent to mention
   the proposal in its closing message, with `acs lore review`. `acs status` already printed
   "Lore proposals from agents: N wait for review (acs lore review)."; the VS Code extension now
   shows the count in the status bar (`$(lightbulb) N`) and a tooltip line.
7. **Option D, documentation only.** `docs/lore.md` gains "How agents are asked to propose" and
   a table of lore against hand-off notes; the usage skill (three copies) says when ACS asks and
   what goes where. `acs init` does not touch `CLAUDE.md` or `AGENTS.md`.

## Evidence

Tests: `the_end_of_a_task_asks_once_for_lore_after_files_changed`,
`with_lore_off_the_end_of_a_task_asks_nothing` and
`stop_through_each_adapter_asks_once_and_then_lets_the_agent_stop` in
`crates/acs-cli/tests/e2e.rs`; `what_an_agent_is_told_follows_every_switch`
(`crates/acs-cli/tests/mcp.rs`) now also checks the start message and the `turn_end` answer
for every switch; adapter unit tests for each `Stop`.

Real agents: `tests/e2e/real/lore_prompt.sh` (new), run 2026-11-20 on Windows, tasks that never
mention lore (add `mul` to `calc.py`; the README says `calc.py` must stay importable by Python
2.7, which the code does not show). All checks passed:

- **Claude Code 2.1.316, Sonnet.** The session that edited was asked once: its transcript holds
  one `Stop` hook answer that blocked (Claude Code records it as `hook_blocking_error`), and the
  session then ended with a result. It loaded `acs_lore_propose` from the start message, proposed
  the Python 2.7 rule scoped to `calc.py`, and its closing message named the proposal and
  `acs lore review`. A second session that only answered a question was not asked. `acs status`
  showed "Lore proposals from agents: 1 waits for review".
- **Codex 0.170.0.** Asked once (the `block` reason reached the model), answered "No additional
  lore to propose. `README.md` already documents the Python 2.7 compatibility requirement", and
  `codex exec` exited cleanly.
- **`agy` 1.5.9, `gemini-3.8-flash-medium`.** Asked once: `{"decision":"continue","reason":...}`
  from `Stop` does reach the model (the adapter doc had this from one source only). It proposed
  the same rule, said so with `acs lore review`, and the run ended.

## Consequences

- Every session that changes files in a repository with lore on spends one more model turn, once.
- The plugins changed: Claude Code picks up the `Stop` hook when the plugin updates; Codex and
  Antigravity repositories need `acs plugin install codex` / `antigravity` again. Reinstall
  `acs`; no state schema change.
- A new canonical phase is a spec change; other adapters written against SPEC 0.1 never send it.

## Not done, not checked

- The desktop apps (Claude Code's and Antigravity's) were not run. Claude Code labels a blocking
  `Stop` answer as a hook error in its transcript; how the app shows it to the person (as an
  error, or as "Stop hook feedback") was not seen.
- Each harness was run once; a run where the model ignores the request and stops was not
  produced, though the tests cover that the next `Stop` gets no answer.
- How often agents actually propose, and whether their proposals are good, is the model's part;
  the real runs show the request arrives, not that lore will grow.
- `acs init` suggesting a line for `CLAUDE.md` / `AGENTS.md` (the rest of option D) was not done.
