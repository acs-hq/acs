---
name: acs-usage
description: How to work alongside other agents under ACS: announce your intent, claim before large changes, read and follow denials, release when done. Use at the start of any coding task in a repository that uses ACS.
---

# Using ACS

ACS (Agent Control System) stops agents in one repository from overwriting each other. Its
tools are on the `acs` MCP server (`acs_status`, `acs_intent`, `acs_claim`, `acs_release`,
`acs_why`).

## At the start of a task

1. Call `acs_status`. It lists the files other agents hold and your own claims.
2. Call `acs_intent` with one sentence about what you are going to do.

## While you work

- Editing a file claims it automatically. If the edit is denied, the message names who holds
  the file and what to do. Work on other files, or ask the user. Never retry the same call.
- For a large change, a rename of many files or a bulk delete, call `acs_claim` for the paths up
  front. Deleting files you have not claimed is blocked.
- Never route around a denial: no shell command, script, copy-and-rename or tool swap that
  reaches the same file. ACS also checks what commands changed.
- Never edit ACS's own files or the files that load it: `hooks.json`, `mcp_config.json`,
  plugin folders, or rules and skills that belong to ACS. They are protected on purpose.

## When you finish

- Call `acs_release` for claims you no longer need. They also expire on their own.
- If something was blocked and you do not understand why, call `acs_why`.
