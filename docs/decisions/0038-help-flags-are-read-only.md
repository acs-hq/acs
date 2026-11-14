# 0038 — Help flags are read-only for self-protection

Date: 2026-11-14. Status: accepted. Decided by: Claude.

## Context

Self-protection (decision 0026) judges an `acs` command by its subcommand. `acs claim --help`
only prints help, but it was denied as `banned_command` like `acs claim`, and the denial took
the rest of that command line with it (issue 60). `acs help claim` worked. Allowing `--help`
must not open a bypass: `acs override --path --help` hands `--help` to `--path` as its value,
and `acs git-hook pre-commit --help` passes `--help` to the hook, because that command takes
the rest of its line as arguments with hyphen values allowed.

## Decision

1. **A command is help-only when everything after `acs` is subcommand names followed by exactly
   one `--help` or `-h` as the last word.** A name is a plain word of letters, digits, `-` and
   `_` that does not start with `-`, so no option can take the next word as its value. Clap
   prints help the moment it reaches the flag, before it reads values or runs anything.
2. **Help-only commands are allowed for every subcommand**, nested ones included
   (`acs lore review --help`), and in the wrapped forms the shell analysis sees through
   (`sudo acs claim --help`, `cargo run -p acs-cli -- claim --help`, `xargs acs claim --help`).
3. **Everything else keeps the old rule:** an option before the flag (`--path --help`,
   `--json --help`), `--`, `--path=--help`, `--help=x`, `-hx`, a word after the flag, a path or
   other operand before it (`acs claim src/lib.rs --help`), or `--help` before the subcommand.
4. **`acs git-hook` is allowed only as `acs git-hook --help`**, since any name after it is a
   hook argument.

## Consequences

- Agents can read the help of any subcommand. The shape is deliberately narrower than what clap
  accepts (`acs claim src/lib.rs --help` would also print help but is denied); that costs
  nothing, since `acs help claim` still works.
- The adversarial suite gained look-alike `deny:` cases and `allow:` help forms. A test runs the
  allowed forms on the built binary and checks exit 0, usage on stdout and no change to the
  repository or the ACS home.
- A new subcommand that takes hyphen-valued trailing arguments, like `git-hook`, must be added
  to the exception in `asks_for_help_only` (`crates/acs-core/src/engine/protect.rs`).
