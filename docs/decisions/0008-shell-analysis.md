# 0008 — Shell analysis (`acs-shell`)

Date: 2026-10-29. Status: accepted (WP1.4).

## Context

The gate has to decide about shell commands without running them. `acs_core::gate` fixes the
output (`ShellAnalysis` and friends); this record covers what `acs-shell` does to produce it
and the places where the plan and the interfaces left room. The analysis is a safety net, not a
sandbox: whatever it cannot see must show up as unknown, never as harmless.

## Decision

### Parsing

- Bash is parsed with `tree-sitter` 0.27 and `tree-sitter-bash` 0.25.1. The grammar crate only
  depends on `tree-sitter-language`, so it works with any tree-sitter runtime that accepts ABI
  15, which 0.27 does. Both compile their C with `cc`, no system libraries.
- A parser is created per `analyze` call (`Parser::new` plus `set_language`: about 0.4 µs). There
  is no global state.
- Syntax deeper than 100 levels, or a tree with errors, is not trusted: errors clear `complete`
  and keep whatever commands were recognized; if none were, the line becomes one unknown command.
  The depth check is iterative, the walk recursive, so hostile nesting cannot overflow the stack
  (tested with 5000 levels on a 2 MiB test thread).
- PowerShell and cmd have a small tokenizer (quotes, escapes, `;`, `|`, `&&`, `||`, `&`,
  redirections, script blocks) and a hand-written table of the cmdlets/commands that delete,
  move, copy and write. Anything else is unknown (unknown `Verb-Noun` cmdlets) or goes through
  the same program table as bash (external tools such as `git`, `npm`, `cargo`). Cmdlet
  parameters match by unique prefix, as in PowerShell (`-Rec`).

### What a command is

- `words` is the *effective* command: leading `NAME=value` assignments and wrappers (`sudo`,
  `doas`, `env`, `nice`, `ionice`, `time`, `command`, `builtin`, `exec`, `nohup`, `setsid`,
  `stdbuf`, `timeout`, `watch`) are removed, and a program given by a system path
  (`/bin/rm`, `/usr/bin/git`) is shown by its name. That is what `destructive.ban` patterns
  see, so `sudo rm -rf /` is caught by `rm -rf /*`. The price: a ban on `sudo` itself cannot be
  expressed (see the contract note below). A program at any other path (`./x.sh`,
  `/opt/tool/run`) is a script run: unknown.
- Nested scripts are analyzed in place: `sh -c '...'` (also `bash -lc`, `zsh`, `fish`...),
  `eval ...`, `trap '...' SIG`, `xargs sh -c`, `find -exec sh -c`, `powershell -Command`,
  `cmd /c`. The shell command itself stays `[unknown]` with `targets_known = false` (as the
  plan requires) and the commands of the script follow it. Consequence: `bash -c 'rm -rf build'`
  is destructive *and* has unknown targets, so the engine's `destructive_unknown_targets`
  applies, which is the intended safe side. Depth is capped at 4 levels. A script that is a
  lone parameter (`sh -c "$CMD"`) or only a command substitution is not re-parsed; the
  substitution is analyzed where it is written.
- `xargs CMD` and `fd -x CMD` keep CMD's class and add `unknown`, with `targets_known = false`
  (`xargs rm` is `[delete, unknown]`). `{}`/`-I` placeholders are not paths.
- `find`: the start paths (default `.`) are the targets, recursive. `-delete` is a delete;
  `-exec CMD {} ;` classifies CMD and maps its use of `{}` onto the start paths; `-fprint` writes.
  The `-name`/`-type` filters are not used to narrow the target: the start path is reported, which
  is a superset.
- Class names follow the plan: read-only commands are `read`; `git commit`, `tag`,
  `branch`... are `write` without targets (they change repository state, not working-tree files);
  commands that change unknown working-tree files (`git merge`, `pull`, `checkout <branch>`,
  `rebase`, `stash`, `cargo test`...) are `write` with `targets_known = false`.
  `GitHistoryRewrite` is added whenever `GitInvocation::rewrites_history` is set; `unknown` always
  implies `targets_known = false`.
- Test runners, build tools (`make`, `npm run`, `pytest`, `docker build`...), interpreters
  (`python x.py`, `node -e`, `perl -e`), `source`, scripts and unknown programs are `[unknown]`.
  `cargo build/check/clippy/doc` are `write` of `target` (known); `cargo test/run/bench` add
  `unknown` because they execute code.

### Targets

- `path` is as written after quote removal. A literal `cd DIR` is tracked: later relative
  paths are joined to it and lexically normalized (`cd sub && rm ../x` gives `x`). The directory
  survives `&&`, `;` and `{ }`, is restored after `( )`, `$( )`, pipelines and function
  bodies, and becomes unknown after a `cd` inside `if`/`for`/`while`/`case`, on the right of `||`,
  after `pushd`/`popd`/`cd -`/`cd` without argument, or when the argument is not literal. With an
  unknown directory, relative targets are `Partial`; absolute ones stay as they are.
  `git -C dir`, `tar -C dir` and `curl --output-dir` are applied the same way.
- `Confidence`: `Pattern` for unquoted `* ? [..]` and brace expansions (`{a,b}`), `Partial` for
  anything with `$`, backticks, `$(..)` or arithmetic, and for a leading `~` (the home
  directory is not known here). Quoted globs are exact (`rm "*.txt"` removes a file of that
  name), except for pathspecs the program expands itself (`git rm "*.log"`, `prettier "src/**"`),
  which are `Pattern`. Process substitutions and placeholders are not paths and are dropped.
- `recursive`: `rm -r`, `cp -r/-a`, `chmod -R`, `find`, `git clean`, extraction targets
  (`tar x` and `unzip` report the destination directory, `.` by default, as a recursive write),
  checkout/restore pathspecs. `mv` marks its sources and destination recursive, and `cp`,
  `ln` and `install` mark the destination recursive (except with `-T`), because a source may be
  a directory and a destination may be a directory the sources land in; the engine can then
  check claims below the path.
- Redirections: `>`, `>>`, `>|`, `&>`, `2>`, `n>` write, `<` reads, `>&2`/`2>&1` are ignored,
  `/dev/null`, `/dev/stderr` etc. are ignored (PowerShell `$null`, cmd `nul` too). A redirection
  on `a && b > f` or `a | b > f` belongs to `b`; on a group, loop or `if` it is attached to the
  first command inside (the group's other commands are not listed as writing it). A
  redirection with no command (`> f`) is the command `:`. A write redirection adds the `write`
  class to its command.
- Heredoc bodies are data, except for `$( )` inside unquoted ones.

### Git

`GitInvocation` follows the issue: global options are skipped (`-C`, `-c`, `--git-dir`,
`--work-tree`, `--no-pager`...), `-C`/`--work-tree` directories rebase the targets. Rewrite is
set for `reset --hard/--merge/--keep` and for `reset` onto another revision, `checkout`/`restore`
of working-tree paths (`checkout -- .`, `checkout .`, `restore .`), `checkout -f/-B`,
`switch -f/-C`, `clean` (not `-n`), `rebase`, `commit --amend`, force pushes and remote ref
deletion, `filter-branch`/`filter-repo`, `stash drop/clear`, `branch -D` (and `-d -f`),
`tag -f`, `branch -f`, `update-ref`, `gc --prune=now`/`prune`, `reflog expire/delete`,
`pull --rebase`. Force push is `-f`, `--force`, `--force-with-lease[=..]`, `--force-if-includes`,
`--mirror` or a `+refspec`. `creates_commit` is `commit`, `merge` (not `--squash`/`--no-commit`),
`cherry-pick`, `revert` (not `-n`), `am`, `rebase`, `pull`, `commit-tree`. `refs` holds push
destinations (`refs/heads/` stripped, `HEAD` kept as written), the revision of `reset`, the
branch of `checkout`/`switch`, `rebase`/`merge` operands and branch/tag names; unexpanded
`$VAR` stay as written. An unknown subcommand (an alias) is `unknown`.

## Consequences

- Known gaps, all in the safe direction or unknown: `time (cmd)`, `time { cmd; }` and
  `! { cmd; }` are not understood by the grammar (the commands are missed, the line may still
  parse); shell variables are not tracked (`D=build; rm -rf $D` is partial); aliases and shell
  functions are not expanded (calling a function shows as an unknown command, its body is
  analyzed where defined); `sed` scripts are split on `;` only approximately; `sed -f`,
  `awk` programs with output redirection/`system`/pipes are `write` + `unknown`; PowerShell
  and cmd handling is heuristic by design; `ssh host CMD` is `network` only (CMD runs remotely).
- Because the wrapper is removed from `words`, policy cannot ban `sudo` or `env` directly. If that
  turns out to matter, `SimpleCommand` needs a `wrappers: Vec<String>` field (contract change
  for the orchestrator, not made here).
- The corpus (`tests/fixtures/corpus.txt`, more than 1300 command lines in bash, PowerShell and
  cmd) is the specification of the table. Adding a program: add its name to the matching arm of
  `table::dispatch` (or a `Sub` table for tools with subcommands), add cases to the corpus,
  run `ACS_SHELL_BLESS=1 cargo test -p acs-shell --test corpus` to record what the analysis
  says, and review the added lines.
- Measured (release, shared 4-CPU machine): 5 µs for `ls`, 10 µs for `cd sub && rm -rf build`,
  28 µs for a three-command pipeline, 48 µs for a 160-byte loop; the parser costs 0.4 µs of that.
  The hook budget is two orders of magnitude above this.
