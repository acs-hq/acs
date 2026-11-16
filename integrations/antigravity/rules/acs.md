---
trigger: always_on
description: How to work with ACS, the Agent Control System that coordinates agents in this repository.
---

# Working with ACS

ACS coordinates the agents (and people) working in this repository. A repository switches on
the parts of ACS it wants, so ACS itself tells you what applies here.

1. Once per conversation, before any other work on the user's first message: call the
   `acs_status` tool (MCP server `acs`), which says which parts of ACS are on in this
   repository and what they ask of you, and do what it says. Then call `acs_intent` with one
   sentence saying what you are about to do. Do this whatever the task is, and even if no hook
   message has told you anything. **Do not call `acs_status` again with each later message.**
   Later in the conversation, call `acs_intent` only when the user gives you a different task,
   and `acs_status` only when you need to know who holds files now.
2. Use the ACS tools you are given. A tool that is not in your tool list belongs to a part of
   ACS that is switched off here: do not look for it and do not try to do its job another way.
3. When a tool call is denied, read the reason. It says what is blocked, who or which rule
   blocks it and what to do next. Follow it: work on other files, or ask the user. Do not retry
   the same call, do not work around the block through a shell command, a script or another
   tool, and do not edit hook, plugin, MCP or rule files to switch ACS off.
4. If you are unsure why something was blocked, call `acs_why`.
5. Follow what ACS adds to your context (for example blocks that start with "ACS lore for").
   Never write `.acs/` yourself.
