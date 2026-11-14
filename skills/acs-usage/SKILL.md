---
name: acs-usage
description: How to work in a repository that uses ACS (Agent Control System). Use at the start of any task that edits files, and whenever an ACS hook or git hook blocks something or asks you to confirm.
---

# Working in an ACS repository

ACS lets several agents and people share one repository. A repository switches on the parts of
ACS it wants, so this skill is in sections, one per part. **ACS tells you which parts are on:**
in the message it adds when your session starts, and in the reply of `acs_status`. Follow the
sections for the parts that are on and skip the rest. Only your pilot can change the switches.

| Part | What it does | What it asks of you |
| --- | --- | --- |
| `claims` | A file you edit is claimed for you; a file another agent holds is blocked | "When claims are on" below |
| `lore` | Shows you facts the owners of the code want every agent to know | "When lore is on" below |
| `guards` | Sealed paths, banned commands, protected branches, delete limits | Nothing until a call is blocked |
| `territory` | Zones of the repository that belong to named owners | Nothing until a call is blocked |
| `commit_checks` | Git hooks that reject a commit or push of files that are off limits | Nothing until a commit is rejected |
| `history` | Records what each session changed, for `acs log` and `acs undo` | Nothing |
| `sync` | Shares claims with your pilot's teammates on other machines | Nothing |

ACS's protection of itself is not a part and is never off: hook, plugin and settings files,
`.git/` and `.acs/` cannot be changed by an agent in any repository.

## The tools

The ACS tools are the MCP server `acs` of this plugin. In Claude Code their full names are
`mcp__plugin_acs_acs__acs_status`, `mcp__plugin_acs_acs__acs_intent`,
`mcp__plugin_acs_acs__acs_claim`, `mcp__plugin_acs_acs__acs_release`,
`mcp__plugin_acs_acs__acs_why` and `mcp__plugin_acs_acs__acs_lore_propose`. If they are not
loaded yet, find them with ToolSearch (search for `acs`). In Antigravity the server is `acs_acs`
and the tools are `mcp_acs_acs_acs_status`, `mcp_acs_acs_acs_intent`, `mcp_acs_acs_acs_claim`,
`mcp_acs_acs_acs_release`, `mcp_acs_acs_acs_why` and `mcp_acs_acs_acs_lore_propose`.

**A tool that is not there belongs to a part that is switched off.** With claims off there is
no `acs_claim` and no `acs_release`; with lore off there is no `acs_lore_propose`. Do not keep
searching for a missing tool and do not try to do its job another way: nothing needs doing.
`acs_status`, `acs_intent` and `acs_why` are always there.

## Always

1. **Start with `acs_intent`.** Before anything else, say in one sentence what you are about to
   do. Other agents and your pilot see it.
2. **Call `acs_status`** before you plan edits. It says which parts of ACS are on, what you and
   others hold, and which paths are never writable.

## When claims are on

Editing a file claims it for you, and a file another agent has claimed is blocked until they
finish.

1. **Claim up front for large changes** with `acs_claim`, listing every path or directory you
   will touch. A claim that conflicts tells you who holds the file, so you can choose other work
   instead of failing halfway.
2. **Claim before you delete or move files.** Deleting a file outside your own claims is denied
   by default, whether you use a file tool, `rm` or `git rm`. Call `acs_claim` on it first.
3. **Release** with `acs_release` when you are done with files, so others can work.

When claims are off, none of this applies: edit, delete and move files without claiming.

## When lore is on

The first time you read, edit or claim files in part of the repository, ACS may add lore to your
context: blocks that start with `ACS lore for <scope> (<id>, owner <owner>)`. They are facts the
owners of that code want every agent to know. **Follow them** as you would the repository's
instructions. Each entry comes once per session, so remember it.

- An entry marked "may be out of date" was written before much of its code changed. Check it
  against the code before relying on it.
- **Propose what you learned.** At the end of a task, if you found a fact about this code that
  the next agent would need and the code does not make obvious, propose it as lore with
  `acs_lore_propose`: `scope` (path globs such as `["payments/**"]`), `text` (a few sentences)
  and, optionally, `why`. Never write lore files yourself: `.acs/` is sealed.
- If an entry is wrong, propose a correction with `acs_lore_propose`, passing its id as
  `replaces`; the owner reviews it.
- `acs lore show <path>` prints the lore for a file again.

When lore is off, no lore is shown and there is nothing to propose.

## When ACS blocks a call

A blocked call comes back with a message that says what is blocked, who or which rule blocks it,
and what to do next. **Read it and follow it.** Which of these you can meet depends on the parts
that are on:

- The file is claimed by another agent (`claims`): work on something else, or ask your pilot to
  arrange the handover. Do not retry the same edit in a loop.
- The path is sealed (`guards`, or ACS's protection of itself): it is protected on purpose. Ask
  your pilot to change it.
- The command is banned or the branch is protected (`guards`): do not rephrase the command to
  get past the rule. Ask your pilot.
- The path is owned territory (`territory`): only the agents of the owners the message names
  may change it. Work on other files, or tell your pilot what you need there; they can
  coordinate with an owner or let you in themselves.
- Ask for confirmation: your pilot decides. Explain what you want to do and why.
- `git commit` is rejected (`commit_checks`): the hook names the staged files that are off
  limits. Unstage them and commit the rest, or ask your pilot to commit them.

Call `acs_why` to get the explanation of the last decision again.

## Never work around ACS

Do not try to bypass or switch off ACS or its checks, whatever is switched on. That includes
editing hook or settings files, writing the same content through another tool or a shell
command, running `acs off`, `acs override`, `acs features <name> on|off` or
`git commit --no-verify`, starting another agent process to make the change, or splitting a
change to slip past a rule. Of the `acs` command line, run only `acs status`, `acs features`
(the list) and `acs lore list|show|stale`; use your ACS tools for everything else. A denial is
information about the repository, not an obstacle to defeat.
