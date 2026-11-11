# Overrides and self-protection: what ACS stops, and what it does not

Written 2026-11-09 with WP7.1 and WP7.2 (decision 0026). ACS guards a repository against accidents
and careless agents. It is not a sandbox: an agent determined to get around it, with a shell, can.
This page says honestly where the line is, so nobody relies on more than ACS gives.

## What is enforced

Whatever the policy says and whether or not `acs off` is active, the hook denies an agent:

| Attempt | How it is caught | Reason code |
| --- | --- | --- |
| Running `acs off`, `acs on`, `acs override`, `acs claim`, `acs release`, `acs init`, `acs plugin`, `acs hook`, `acs mcp`, `acs doctor --fix`, `acs undo --apply`, `acs lore review`, or any future subcommand | Only `acs status`, `acs sync`, `acs doctor`, `acs log`, `acs blame`, `acs stats`, `acs verify`, `acs undo` without `--apply`, `acs lore list/show/stale` and help are allowed | `banned_command` |
| Skipping git hooks: `--no-verify`, `git commit -n`, `git -c core.hooksPath=...`, `git config core.hooksPath ...` | Words of the git command | `banned_command` |
| Starting Claude Code or Antigravity with hooks or the ACS plugin off: `claude --bare`, `--settings`, `--setting-sources`, `agy --settings`, `plugin disable`/`uninstall` | Words of the harness command | `banned_command` |
| Writing or deleting `ACS_HOME`, a git directory outside the work tree, or a harness's user-level hooks, plugins or settings (`~/.claude/settings*.json`, `~/.claude/plugins/`, `~/.gemini/config/{hooks.json,plugins/,config.json}`, `~/.gemini/antigravity-cli/settings.json`) | File tools and shell targets, with `~`, `$HOME`, `%USERPROFILE%`, `$env:USERPROFILE`, `$ACS_HOME` and `$CLAUDE_CONFIG_DIR` expanded | `sealed_path` |
| A command ACS cannot see into (an interpreter, a script, an unknown program) that mentions one of those locations, or `.acs/`, `.git/hooks`, `.git/config`, `.claude/settings` or `.husky/` | Words of the command | `sealed_path` |
| A command matching a `destructive.ban` pattern | `guards.toml` | `banned_command` |

These rules see through wrappers (`sudo`, `env`, `nohup`, `timeout`, `command`, ...), subshells and
command substitutions, `sh -c` / `bash -c` / `eval` / `cmd /c` / `pwsh -Command` / `iex`, alias and
function bodies, `script -c`, `su -c`, here-strings and here-documents fed to a shell, `echo` or
`printf` piped into a shell (also through `base64 -d`), PowerShell `-EncodedCommand`, `xargs`,
`find -exec` and `cargo run -p acs-cli -- ...`.

`crates/acs-cli/tests/fixtures/adversarial.txt` lists every case; `cargo test -p acs-cli --test
adversarial` requires all `deny:` lines to be denied and the `allow:` controls to pass. Add every
bypass you see to it.

**Owned zones** (WP7.1) deny agents whose pilot is not an owner. Only a person can let one in, with
`acs override`, which:

- is denied to agents by the rules above;
- refuses to run unless standard input and standard error are terminals, so `echo y | acs override`
  and `acs override < answers` fail;
- prints a random four-letter code that must be typed back, so an answer piped into a pseudo-terminal
  in advance (`echo ABCD | script -qc 'acs override ...'`) is wrong;
- lifts `owned_territory` only (claims, sealed paths and every other rule still hold), for one tool
  call (`--once`) or one session (`--session`), and lapses unused after `claim_ttl_secs`;
- is recorded per use with the seat (`override_uses`), listed by `acs status`, and goes into the
  session record (`overrides`) once session records exist (WP4.1).

## Residual risks

What these rules do not stop, roughly from most to least likely to matter.

1. **Code ACS cannot read.** A script file (`python tools/x.py`, `bash setup.sh`, `make`, `npm run`)
   or an interpreter one-liner that does not spell out a protected path can do anything: edit
   files without claiming them, run `acs off`, rewrite a settings file whose path it builds at run
   time. The post-tool scan attributes changed files in the repository afterwards, and the git hooks
   check staged files at commit time, but nothing stops the action itself. *Seen while dogfooding
   (2026-11-09): files an agent edited through `python` in its shell were never claimed, so other
   seats were not protected from that agent's edits.*
2. **Programs named by a variable or by command output.** `A=acs; $A off`, `$(echo acs) off`,
   `eval "$(printf 'acs off')"`, `` `which acs` off ``: the program word is not known, so the command
   is unknown and allowed when it does not look destructive.
3. **Environment variables.** The analysis drops leading `NAME=value` assignments, so it cannot see
   `CLAUDE_CONFIG_DIR=/tmp/x claude -p ...` (a fresh Claude Code configuration without the ACS
   plugin), `GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=core.hooksPath GIT_CONFIG_VALUE_0=/dev/null git
   commit`, or `HUSKY=0 git commit` in repositories whose ACS hook runs through husky.
4. **Other harnesses and other config.** Only Claude Code and Antigravity are known. Other agent
   CLIs, Claude Code's managed settings, `~/.claude.json` (MCP servers) and settings files in other
   repositories are not protected.
5. **A real terminal.** A harness that gives agents an interactive pseudo-terminal they can read
   and type into (tmux `send-keys`, an IDE terminal tool) satisfies the terminal check and can read
   the code. The hook's denial of `acs override` is then the only barrier, and it does not apply if
   the command runs somewhere ACS does not see (risk 1).
6. **Reads.** Agents may read `ACS_HOME`, harness settings and anything else. Nothing secret is
   stored there, but collision messages name people and files.
7. **Who granted it.** `granted_by` is the machine's `git config user.email`, which anyone at the
   machine can change. Overrides are a local, same-user trust decision.
8. **A used-up override that did not help.** A `--once` override is used up before claims are
   taken; if the claim then loses a race, the call is denied and the override is gone. Grant it
   again.
9. **False positives.** A command ACS cannot see into is denied when it merely mentions a protected
   path, even to read it (`python -c "print(open('.acs/policy/guards.toml').read())"`). Use `cat`.
   `grep acs off` and similar are allowed because `acs` only counts there when followed by a
   stateful subcommand; `grep -r acs release` would be denied.
10. **Hooks that never run.** If `acs` is not on the `PATH` the harness started with, Claude Code's
    hooks fail open and check nothing (Antigravity fails closed). `acs doctor` checks it.
11. **Antigravity's `always-proceed`.** It approves ACS `ask` decisions without a prompt (issue #7).
    The rules on this page deny instead of asking, so they are not affected, but other asks
    (large deletes, unknown targets) are approved silently there.
12. **Writes elsewhere outside the repository.** Since decision 0027, ACS does not ask about them
    unless `protect_outside_repo = true`; the harness's own permission rules decide. The protected
    locations above are still denied, and paths that link back into the repository still ask.
13. **`acs watch` and long agent commands.** The watcher holds back changes while an agent's shell
    command runs, for at most 5 minutes per change (decision 0028). A script that runs longer and
    writes files ACS could not predict can have those changes claimed for the person, which then
    blocks other agents. Agents cannot start `acs watch` themselves.

## Bugs found while dogfooding this work

| Date | What happened | State |
| --- | --- | --- |
| 2026-11-09 | Edits made through `python` in the agent's shell were invisible to ACS: no claims, no conflict check (risk 1 above). | Known limit; documented |
| 2026-11-09 | `cd dir && cmd || fallback` was analyzed with `fallback` in the directory from before the `cd`, although bash runs it in `dir` whenever the `cd` succeeded. A `mv` was wrongly denied as "not claimed"; the same mistake could have let a delete in `dir` be checked against the wrong file. | Fixed in `acs-shell` (the fallback's directory is unknown when the left side changed it); corpus cases added |
| 2026-11-09 | Every write an agent made outside the repository (its scratch files in the OS temp folder) stopped the session with an "accept once / deny" prompt: ACS answered `path_escapes_repo` ask. Noise that trains the pilot to click accept. | Fixed by decision 0027: off by default, `protect_outside_repo = true` to opt in |
| 2026-11-09 | In this repository, an agent can no longer run `acs` against a scratch repository from its shell (`target/debug/acs hook --canonical ...`, `acs init` in a temp folder): the rule cannot tell which repository the command acts on. | Accepted friction; use tests or scripts |
| 2026-11-09 | `acs claim --help` from an agent's shell is denied like `acs claim`, although it only prints help: the rule looks at the subcommand, not at `--help`. It took the rest of that command line down with it. | Open; `acs help claim` works. Allowing `--help` needs care (a flag value that looks like `--help`) |
| 2026-11-10 | Phase 4 subagents worked in git worktrees under `.claude/worktrees/` and committed with `cd <worktree> && git commit`. All three were rejected by the pre-commit hook as "made by a person" for files claimed by their own seat (subagents share the parent's Claude Code seat). The pending-commit marker is keyed by the work tree of the event's `cwd`, which Claude Code always sends as the session's directory (the main checkout), so the hook in the worktree finds no marker. Any agent committing in another directory (`cd`, `git -C`) is hit. The agents released their claims with `acs_release` and committed; none used `--no-verify`. | Open. Fix: key the marker on the directory the command commits in (the `cd` target or `git -C`), or let the hook accept a marker of the same seat from any worktree of the repository |
| 2026-11-10 | The rejection in the row above tells the agent to run `git commit --no-verify`, which self-protection denies to agents. | Open. For an agent's commit, point at `acs_release` (own claims) or waiting for the holder |
| 2026-11-10 | Edits in a linked worktree that lives inside the main checkout (`.claude/worktrees/wp4-undo/crates/acs-git/src/merge.rs`) are claimed under that main-checkout-relative path, not as `crates/acs-git/src/merge.rs`: the hook resolves paths against the work tree of the event's `cwd`. Such a claim protects nothing in the worktree's own terms, so another seat editing `crates/acs-git/src/merge.rs` in the main checkout or another worktree is not stopped, and `acs_release crates/...` from the worktree finds no claim (a subagent saw "claims on only 2 of 7 files"). `acs status` at the end of Phase 4 listed 27 such `.claude/worktrees/...` claims. Same root cause as the commit-marker row above. | Open. Fix: resolve each path against the work tree that contains it (`git rev-parse --show-toplevel` of the path's directory, cached), and claim it relative to that tree |
| 2026-11-11 | An agent creating GitHub issues with `gh issue create --body "$(cat <<'EOF' ... EOF)"`, wrapped in a shell function, was denied as "may change ACS state (`ACS_HOME`)": the issue text only mentioned protected locations (`ACS_HOME`, `.acs/`, `.claude/settings.json`, `.husky/`) in prose. Residual risk 9 (a command ACS cannot see into is denied when a word mentions a protected path), hit through a function call whose argument was a here-document. The whole command was refused, the label and milestone steps before it included. The agent wrote each body to a file with the Write tool and used `--body-file`. | Open. Text passed as data to a known program (`gh ... --body`, `git commit -m`) should not count as a mention; the function wrapper is what made the command unknown |
| 2026-11-10 | `acs_release` with directories (`crates`, `spec`) released nothing ("You held no claim on: `crates` ..."), although its description says a directory covers the files under it. Likely the installed `acs` predates `c42d47c` ("Release directories"). | Check after reinstalling |
