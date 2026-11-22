# 0052 — `acs plugin install` without the harness's command line

Date: 2026-11-22. Status: accepted.

Decided by: the human, that all three harnesses must work for someone who uses only a desktop
app, preferring the app's own plugin screen to new code where that is enough; Claude, for how.

## Context

Issue 83: someone who uses Claude Code only in the Claude desktop app has no `claude` on `PATH`,
so `acs plugin install claude-code` stopped with `harness_not_found`. Antigravity needed `agy` for
the same reason, although `agy` only validated a folder ACS had already written. Codex already
worked without `codex`.

The Claude Code docs (plugins/install and plugins/loading, read 2026-11-22) say the desktop
app's local sessions read the same settings files as the CLI, and install plugins from **+ →
Plugins → Manage plugins**, with a choice of scope. They also say Claude Code does not download a
plugin that only the committed `.claude/settings.json` enables: each machine must install it once.
An enable in the untracked `settings.local.json` or in the user settings is fetched by itself,
after the folder is trusted. That makes the issue's option 1 a full fix for local scope only.

## Decision

1. **Claude Code, project and local scope, no `claude`:** ACS writes
   `extraKnownMarketplaces."acs-plugins"` (the `--marketplace` source in Claude Code's shape: a
   GitHub `owner/repo`, a git URL, or an absolute folder) and `enabledPlugins."acs@acs-plugins"`
   into `.claude/settings.json` or `.claude/settings.local.json`. The rest of the file keeps its
   text and order (`commands/claude_settings.rs`, shared with `acs uninstall`, which removes exactly
   these two entries). The report has a new `to_finish` field and the text says what is left: for
   project scope, the desktop app's plugin screen once per machine; for local scope, a session
   start in a trusted folder.
2. **Claude Code, user scope, no `claude`:** still `harness_not_found`, now with the desktop
   app's steps. ACS does not edit `~/.claude/settings.json` itself: it would gate every repository
   the person opens.
3. **Antigravity, no `agy`:** the plugin folder is written and the validation is skipped, with a
   warning.
4. **Codex:** no change in what it writes. The closing text adds that the Codex app reads the
   same file, and that the hooks can be reviewed once in the CLI's `/hooks` if the app never
   offers to.
5. The desktop app route needs no ACS code at all: someone can install the plugin from the app's
   plugin screen. The README, the troubleshooting page and the setup skill say so.

## Consequences

- No harness's command line is required any more. `harness_not_found` remains only for Claude
  Code at user scope.
- Whether each desktop app really loads the hooks and MCP tools this way is not checked yet: the
  desktop app's marketplace screen, the Codex app's hook trust, and Antigravity's app with
  `.agents/plugins/` all need a person in the app (a `human-task` issue).
- `acs init` and `acs doctor` still detect Claude Code only by `claude` on `PATH` or a `.claude/`
  folder (issue 28 covers `doctor`).
