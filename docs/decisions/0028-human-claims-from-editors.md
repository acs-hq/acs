# 0028 — Human claims from editors: the VS Code extension and `acs watch`

Date: 2026-11-09. Status: accepted (WP7.3, WP7.4; the dependency and the release rule are human
decisions).

## Context

Until now a person's own edits were invisible to ACS unless they ran `acs claim` by hand, and
those claims were never refreshed. WP7.3 adds a VS Code extension and WP7.4 a file watcher for
other editors. Both must claim what the person changes and nothing an agent changes: a watcher
that claimed an agent's edit as the person's would block other agents for the wrong reason, and a
claim on a file the agent is mid-way through would collide with its own next edit.

## Decision

1. **Both claim for the human seat through the existing operations.** The extension runs
   `acs claim --json` and `acs release --json`; `acs watch` calls the same `claim_paths` and
   `release_paths` in process. A human claim is judged exactly like a CLI claim: territory does not
   bind it, and a refusal on a sealed path is not shown, because sealed paths bind agents, not
   people.
2. **Release rule (human decision): saved, then idle.** A claim is released once its file is saved
   and has gone `releaseAfterIdleSeconds` / `--idle` (default 120) without a change, or at once
   when a saved file is closed in VS Code. Unsaved changes keep the claim. The extension refreshes a
   held claim every five minutes so it does not lapse at `claim_ttl_secs`; `--idle` is capped at
   `claim_ttl_secs` for the watcher, which needs no refresh then.
3. **The extension claims only edits that leave the buffer dirty.** VS Code also reports a reload
   from disk as a change; an agent's edit to an open file reaches the editor that way and leaves
   the buffer clean.
4. **`acs watch` attributes a change to an agent** when an agent seat on this machine holds a live
   claim on the file (agents claim before a file tool writes), or when an agent's post-tool
   attribution of the file is at most 5 seconds older than the moment the watcher first saw the
   change. While any live agent seat has a tool marker waiting for its post-tool step (a shell,
   delete or move call is running), changes are held back, up to 5 minutes per change, because
   that call's post-tool scan may attribute them. Gitignored paths (`git check-ignore --no-index`)
   and `.git/` are never claimed.
5. **The `notify` crate (human decision)** provides the file events (8.2, CC0-1.0, already allowed
   by `deny.toml`), rather than polling `git status`, which costs CPU in proportion to the
   repository.
6. **Agents may not run `acs watch`** (self-protection, decision 0026): it is not on the read-only
   allowlist, and it joins the stateful subcommands checked when `acs` is an argument.

SPEC section 6.2 states the rule for editor integrations and watchers.

## Consequences

- The plan's M4 check "a human editing in VS Code blocks an agent edit" is covered by the
  extension's CLI client test against the real binary and by `acs watch`'s tests; the in-editor
  run is the human's (`docs/releases/wp7-humans.md`).
- Releasing after two idle minutes gives agents the file back before the person commits. A person
  who wants it longer raises the setting or runs `acs claim`.
- A change the person makes to a file an agent holds is not claimed (the watcher cannot tell it
  from the agent's); the extension warns, the watcher prints a note at most every ten minutes.
- A long-running agent shell command delays the watcher's claims until it finishes, at most 5
  minutes. An opaque script that runs longer and writes elsewhere can have its changes claimed for
  the person (`docs/security/overrides.md`, residual risks).
- Stopping `acs watch` or closing VS Code mid-edit leaves claims to lapse after `claim_ttl_secs`;
  VS Code releases what it holds on a clean shutdown.
- Publishing the extension (Marketplace, Open VSX) needs a publisher account and tokens: the
  human's. The `publisher` field (`acs-hq`) is a placeholder until then. CI does not build or test
  the extension yet, and `.github/workflows/` is sealed for agents.
