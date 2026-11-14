# 0034 — What agents are told follows the feature switches

Date: 2026-11-14. Status: accepted. Decided by: the human for the rule (the `acs` program says
what depends on a switch, the plugin's files do not) and for covering every switch, not only
`claims`; Claude for which tools go with which switch, the wording, and the two denial messages.

## Context

Decision 0033 made every feature switchable, but what an agent is told about ACS mostly came
from files that are the same in every repository: the MCP server's instructions, the tool list
and descriptions, the Antigravity rule and the usage skill. Only the session-start message and
the replies of the tools read the switches.

Example: a repository sets `claims = false`. An agent starting a session there was told "claim
files with acs_claim before you edit" by the server, saw an `acs_claim` tool that said the same,
loaded a skill that said the same, and got one session-start line saying claims are off. It
usually called `acs_claim` and was told "claims are switched off". Nothing broke, but three
texts were wrong and one call was wasted. 0033 listed this under "Consequences".

## Decision

1. **Files shipped with a plugin say only what is true in every repository. What depends on a
   switch is said by `acs`,** which reads `config.toml` each time it starts. So the plugin files
   never have to follow the switches, and there is no second copy of the switches to keep in
   step.
2. **The MCP tool list follows the switches.** `acs_claim` and `acs_release` are listed only
   where `claims` is on, `acs_lore_propose` only where `lore` is on. `acs_status`, `acs_intent`
   and `acs_why` are always listed. The other five switches take no tool away: none of the tools
   belongs to them.
3. **The MCP instructions are built from the switches** and name no tool that is not listed.
   With claims off they say that files need no claim.
4. **`acs_status` says what is off.** One line names the other switched-off parts (`lore`,
   `guards`, `territory`, `history`, `commit_checks`); `sync` is left out because it is not the
   agent's concern and is off by default. With claims off the claim lists are replaced by one
   line saying so. With claims on the reply now carries the rule the Antigravity rule used to
   state: claim a file before deleting or moving it.
5. **The session-start message names no tool that is not listed** (it used to say that
   `acs_claim` and `acs_release` are not needed).
6. **Two denials read the switches.** An agent that runs `acs claim` or `acs lore review` from
   its shell is still denied, but is pointed at `acs_claim` or `acs_lore_propose` only where
   that tool exists. This changed `acs-core`'s engine messages, not its public types.
7. **The Antigravity rule names only `acs_status`, `acs_intent` and `acs_why`** and tells the
   agent that a tool missing from its list belongs to a part that is off.
8. **The usage skill is one file in sections, one per part,** with a table of the seven parts
   and what each asks of the agent. It still names every tool (the agent needs the full names),
   and says that a missing tool means its part is off.
9. **It is decided when the harness asks,** which is at session start. A session that started
   before a switch changed keeps its tool list; a call to a tool that is no longer listed still
   answers that its feature is off (0033's replies stay).

## Consequences

- An agent in a repository with claims off never learns that `acs_claim` exists. In Antigravity,
  where ACS tools are always loaded, this also saves their schemas from every context.
- `acs_mcp::Backend` gained `available()` with a default of "everything on", so other backends
  and the tests' fake did not change. `StatusReply` gained `claims_off` and `also_off`.
- The Claude Code plugin is cached by commit, so the reworded skill reaches a user when the
  plugin updates; Antigravity's copy is written by `acs plugin install antigravity` and needs
  that run again.
- A new switchable feature with a tool of its own adds a field to `Available` and a row to the
  test `what_an_agent_is_told_follows_every_switch` (`crates/acs-cli/tests/mcp.rs`), which fails
  if anything an agent is told names a tool that is not listed.
- This replaces the last item of 0033's "Consequences".

## Not done

- **No real-agent run.** The tests speak MCP to the built binary for every switch, but no Claude
  Code or Antigravity session was run to see that an agent no longer looks for `acs_claim`.
- **The VS Code extension** still tries to claim with claims off (issue 71). It is a person's
  tool, not an agent's, and was not touched.
- **Hooks still start for every tool call** (issue 70). The same rule argues against hook
  registration that follows the switches: it would be a second copy of them, and Claude Code
  fails open without a message when a hook is missing. That is a recommendation in the issue,
  not a decision.
