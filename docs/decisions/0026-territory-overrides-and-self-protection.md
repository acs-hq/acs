# 0026 — Owned territory, human overrides and ACS self-protection

Date: 2026-11-09. Status: accepted (human decisions on the four open questions; WP7.1 and WP7.2
built on the branch `wp7-territory-overrides`).

## Context

WP7.1 asked the engine to stop treating `owned` zones as `shared`, and for `acs override --path
<glob> --once|--session` to let a human lift that, recorded in the ledger. WP7.2 asked for agents
to be denied the ways of switching ACS off, an interactive terminal and typed confirmation for
overrides, and an adversarial suite that is entirely denied.

Four questions needed the human (issues acs-hq/acs#16, #48, #12, #13, and the WP4.1 gap):

1. #16: banned commands were allowed while `acs off` was active, but SPEC section 11 says "always".
2. WP7.1's acceptance needs `acs log` and session records, and WP4.1, which builds them, is not
   built.
3. #13: what to do when an agent starts another agent CLI.
4. #12 and decision 0021: which `acs` subcommands an agent may run from its shell.

## Decision

**Territory (WP7.1).** In an `owned` zone, an agent whose pilot is not an owner (teams expanded,
email compared ignoring ASCII case) is denied with `owned_territory`; the message names the zone,
its owners and that the pilot can let it in. Owners' agents still need claims. Territory binds
agents only (`GateInput::origin`); a human `acs claim` is not checked. Territory is enforced when
the state database is unavailable (it needs only policy), and not while the gate is off.

**Overrides.** `acs override --path <glob> --once|--session` stores an override (migration 0005):
`once` is used up by the first tool call that relies on it; `session` binds to the first seat that
relies on it and lasts as long as that seat. Unused overrides lapse after `claim_ttl_secs`.
`--revoke <id>` withdraws one. An override lifts `owned_territory` only. The engine sees overrides
through a new `OverrideView` trait (globs are compiled in the policy dialect by the runtime) and
returns the ones it relied on (`GateOutput::overrides_applied`); `run_gate` records the use
atomically (`StateDb::apply_overrides`) and decides again if another seat won the override. Every
use is stored per path in `override_uses`.

**WP4.1 gap (human: store now, log later).** Overrides and their uses are in the state database and
in `acs status`. `SessionRecord` gains an optional `overrides` field (`OverrideUse`), and
`StateDb::override_uses_for_seat` returns exactly what WP4.1 must put there. "Every override appears
in `acs log`" moves to WP4.1's acceptance.

**Granting needs a person.** `acs override` refuses unless standard input and standard error are
terminals, then prints a random four-letter code that must be typed back. Piped answers fail the
terminal check; pre-piped answers to a pseudo-terminal (`script`) fail the random code. The hook
denies agents the command anyway; this is the second line, for agents ACS does not see.

**Self-protection (WP7.2).** New engine rules (`crates/acs-core/src/engine/protect.rs`), enforced
whatever the policy and the gate state:

- **The `acs` command line (human: read-only allowlist).** Agents may run `acs status`, `acs sync`,
  `acs doctor` without `--fix` and help; everything else, including future subcommands, is denied
  as `banned_command`. When `acs` is an argument of another program (`xargs acs release`), only the
  known stateful subcommands count, so `grep -r acs src` is not caught. This settles decision
  0021's open point: in an adopted repository, agents may not run `acs plugin install` (or `acs
  init`); setup is unaffected because the gate is not active until the plugin loads.
- **Git hooks.** `--no-verify`, `git commit -n`, and setting `core.hooksPath` (for one command or
  in the config) are denied.
- **Nested harnesses (human: deny only hook-disabling forms).** `claude --bare`, `--settings`,
  `--setting-sources`, `agy --settings`, and `claude|agy plugin disable|uninstall` are denied.
  Plain nested runs are allowed: their own hooks gate them.
- **Protected locations outside the repository** (`OutsideSeal`, supplied by the runtime):
  `ACS_HOME`, git directories outside the work tree, Claude Code's user settings and plugins
  (honouring `CLAUDE_CONFIG_DIR`), Antigravity's user hooks, plugins, plugin settings and CLI
  settings. Writes and deletes there are `sealed_path`, also from file tools (which used to get
  `path_escapes_repo`, an ask Antigravity's `always-proceed` approves silently, issue #7). Shell
  targets starting with `~`, `$HOME`, `%USERPROFILE%`, `$env:USERPROFILE`, `$ACS_HOME` or
  `$CLAUDE_CONFIG_DIR` are expanded for this check only; other writes outside the repository keep
  their old behaviour.
- **Commands ACS cannot see into** (class `unknown`: interpreters, scripts) are denied as
  `sealed_path` when a word mentions a protected location, or `.acs/`, `.git/hooks`, `.git/config`,
  `.claude/settings` or `.husky/`.

No new reason code: self-protection uses `banned_command` and `sealed_path`, whose spec rows now say
so (the reason-code set is closed, and both mean "always denied").

**Banned commands while off (human: deny always).** Bans now hold while the gate is off, as SPEC
section 11 always said. Closes #16.

**Shell analysis sees more** (`acs-shell`, benefiting every guard): alias bodies, `script -c`,
`su -c`/`runuser -c`, PowerShell `-EncodedCommand` and `Invoke-Expression` strings, and scripts fed
to a shell on stdin (`echo ... | sh`, through `base64 -d`, here-strings and here-documents) are
analyzed as nested scripts.

**`SimpleCommand.wrappers` (#48) is not needed.** The shell analysis already strips wrappers from
`words`, so self-protection sees `sudo acs off` as `acs off`. The field remains a candidate for
user ban patterns such as `sudo *`.

## Consequences

- Contract changes, made by the orchestrator: `OverrideId`, `OverrideMode`, `OverrideUse`,
  `SessionRecord.overrides` (schema regenerated), and in `gate.rs` `GateInput::{origin, overrides,
  outside_sealed}`, `GateOutput::overrides_applied`, `OverrideView`, `NoOverrides`,
  `OverrideApplied`, `OutsideSeal`. SPEC sections 3.2, 4.3, 6.1 (new), 9, 11 and 11.1 (new).
- State schema version 5. An older `acs` build reports the state as unavailable once a newer one
  has migrated it, so everyone on a machine should update together.
- In this repository, agents can no longer drive `acs` against scratch repositories from their
  shell (`target/debug/acs hook --canonical ...`, `acs init` in a temp folder): the rule cannot tell
  which repository the command acts on. Tests and scripts are unaffected.
- Residual risks, and the bugs found while dogfooding this work, are in
  `docs/security/overrides.md`.
- Issues: closes #16, #12, #13 and #14 (residual cases in the security notes); #48 stays open
  for its other items; #7 stays open (ACS `ask` under `always-proceed` in general).
