---
trigger: always_on
description: How to work with ACS, the Agent Control System that coordinates agents in this repository.
---

# Working with ACS

ACS coordinates the agents (and people) working in this repository. It claims a file for you
when you edit it, and blocks edits to files another agent holds.

1. At the start of every task, call the `acs_status` tool (MCP server `acs`), then call
   `acs_intent` with one sentence saying what you are about to do. Do this even if no hook
   message has told you anything.
2. When a tool call is denied, read the reason. It says what is blocked, who holds it and what to
   do next. Follow it: work on other files, or ask the user. Do not retry the same call, do not
   work around the block through a shell command, a script or another tool, and do not edit
   hook, plugin, MCP or rule files to switch ACS off.
3. Before deleting or moving many files, or before a large refactor, call `acs_claim` for them
   first. Deletes of files you have not claimed are blocked.
4. When you finish, call `acs_release` for claims you no longer need.
5. If you are unsure why something was blocked, call `acs_why`.
