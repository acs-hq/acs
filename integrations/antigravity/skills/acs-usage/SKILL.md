---
name: acs-usage
description: How to work in a repository that uses ACS (Agent Control System). Use at the start of any task that edits files, and whenever an ACS hook or git hook blocks something or asks you to confirm.
---

# Working in an ACS repository

ACS lets several agents share one repository. Editing a file claims it for you; a file that
another agent has claimed is blocked until they finish. Some paths (hooks, settings, CI, the ACS
policy) are sealed and nobody's agent may change them.

The ACS tools are the MCP server `acs` of this plugin. In Claude Code their full names are
`mcp__plugin_acs_acs__acs_status`, `mcp__plugin_acs_acs__acs_intent`,
`mcp__plugin_acs_acs__acs_claim`, `mcp__plugin_acs_acs__acs_release`,
`mcp__plugin_acs_acs__acs_why` and `mcp__plugin_acs_acs__acs_lore_propose`. If they are not
loaded yet, find them with ToolSearch (search for `acs`). In Antigravity the server is `acs_acs`
and the tools are `mcp_acs_acs_acs_status`, `mcp_acs_acs_acs_intent`, `mcp_acs_acs_acs_claim`,
`mcp_acs_acs_acs_release`, `mcp_acs_acs_acs_why` and `mcp_acs_acs_acs_lore_propose`.

## Do this

1. **Start with `acs_intent`.** Before anything else, say in one sentence what you are about to
   do. Other agents and your pilot see it.
2. **Call `acs_status`** to see who holds which files before you plan edits.
3. **Claim up front for large changes** with `acs_claim`, listing every path or directory you
   will touch. A claim that conflicts tells you who holds the file, so you can choose other work
   instead of failing halfway.
4. **Claim before you delete or move files.** Deleting a file outside your own claims is denied
   by default, whether you use a file tool, `rm` or `git rm`. Call `acs_claim` on it first.
5. **Release** with `acs_release` when you are done with files, so others can work.
6. **Propose what you learned.** At the end of a task, if you found a fact about this code that
   the next agent would need and the code does not make obvious, propose it as lore with
   `acs_lore_propose`: `scope` (path globs such as `["payments/**"]`), `text` (a few sentences)
   and, optionally, `why`. Never write lore files yourself: `.acs/` is sealed.

## Lore

The first time you read, edit or claim files in part of the repository, ACS may add lore to your
context: blocks that start with `ACS lore for <scope> (<id>, owner <owner>)`. They are facts the
owners of that code want every agent to know. **Follow them** as you would the repository's
instructions. Each entry comes once per session, so remember it.

- An entry marked "may be out of date" was written before much of its code changed. Check it
  against the code before relying on it.
- If an entry is wrong, propose a correction with `acs_lore_propose`, passing its id as
  `replaces`; the owner reviews it.
- `acs lore show <path>` prints the lore for a file again.

## When ACS blocks a call

A blocked call comes back with a message that says what is blocked, who or which rule blocks it,
and what to do next. **Read it and follow it.** Typical outcomes:

- The file is claimed by another agent: work on something else, or ask your pilot to arrange
  the handover. Do not retry the same edit in a loop.
- The path is sealed: it is protected on purpose. Ask your pilot to change it.
- The path is owned territory: only the agents of the owners the message names may change it.
  Work on other files, or tell your pilot what you need there; they can coordinate with an owner
  or let you in themselves.
- Ask for confirmation: your pilot decides. Explain what you want to do and why.
- `git commit` is rejected: the hook names the staged files that are off limits. Unstage them and
  commit the rest, or ask your pilot to commit them.

Call `acs_why` to get the explanation of the last decision again.

## Never work around ACS

Do not try to bypass or switch off ACS or its checks. That includes editing hook or settings
files, writing the same content through another tool or a shell command, running `acs off`,
`acs override` or `git commit --no-verify`, starting another agent process to make the change, or
splitting a change to slip past a rule. Of the `acs` command line, run only `acs status` and
`acs lore list|show|stale`; use your ACS tools for your own claims and lore proposals. A denial
is information about the repository, not an obstacle to defeat.
