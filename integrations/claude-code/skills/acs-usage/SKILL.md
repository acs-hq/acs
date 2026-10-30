---
name: acs-usage
description: How to work in a repository that uses ACS (Agent Coordination Standard). Use at the start of any task that edits files, and whenever an ACS hook blocks a tool call or asks you to confirm.
---

# Working in an ACS repository

ACS lets several agents share one repository. Editing a file claims it for you; a file that
another agent has claimed is blocked until they finish. Some paths (hooks, settings, CI) are
sealed and nobody's agent may change them.

The ACS tools are the MCP server `acs` of this plugin. In Claude Code their full names are
`mcp__plugin_acs_acs__acs_status`, `mcp__plugin_acs_acs__acs_intent`,
`mcp__plugin_acs_acs__acs_claim`, `mcp__plugin_acs_acs__acs_release` and
`mcp__plugin_acs_acs__acs_why`. If they are not loaded yet, find them with ToolSearch
(search for `acs`).

## Do this

1. **Start with `acs_intent`.** Say in one sentence what you are about to do. Other agents and
   your pilot see it.
2. **Call `acs_status`** to see who holds which files before you plan edits.
3. **Claim up front for large changes** with `acs_claim`, listing every path or directory you
   will touch. A claim that conflicts tells you who holds the file, so you can choose other work
   instead of failing halfway.
4. **Claim before you delete files.** Deleting files outside your own claims is denied by
   default, so call `acs_claim` on them first.
5. **Release** with `acs_release` when you are done with files, so others can work.

## When ACS blocks a call

A blocked call comes back with a message that says what is blocked, who or which rule blocks it,
and what to do next. **Read it and follow it.** Typical outcomes:

- The file is claimed by another agent: work on something else, or ask your pilot to arrange
  the handover. Do not retry the same edit in a loop.
- The path is sealed: it is protected on purpose. Ask your pilot to change it.
- Ask for confirmation: your pilot decides. Explain what you want to do and why.

Call `acs_why` to get the explanation of the last decision again.

## Never work around ACS

Do not try to bypass or switch off ACS or its checks. That includes editing hook or settings
files, writing the same content through another tool or a shell command, starting another
agent process to make the change, or splitting a change to slip past a rule. A denial is
information about the repository, not an obstacle to defeat.
