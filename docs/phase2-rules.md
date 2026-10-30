# Phase 2 rules for work-package subagents

You implement exactly one work package (WP) of the ACS implementation plan. The orchestrator
reviews and merges your branch. Read this whole file first.

## Reading list (before writing code)

1. The implementation plan: `/root/.claude/uploads/441d9e3f-a42d-5cd7-a4aa-bebd95a5bd32/41d669e4-ACS___Implementation_Plan.md`
   — sections 0, 3, 4, 5, 9 and your WP in section 6.
2. `docs/HANDOFF.md` (especially section 3, "Must-know items for Phase 2").
3. `spec/SPEC.md`.
4. All of `docs/decisions/` — **0002, 0004, 0009 and 0011 are required**; 0011 fixes the
   Phase 2 crate roles, CLI surface and cross-WP interfaces you must follow.
5. The public APIs of the crates you consume (read their `lib.rs` and module docs).

## Ownership

- Edit only the paths your WP owns (listed in your brief), plus tests for them and your decision
  record.
- Shared files may only **gain** entries: root `[workspace.dependencies]`, `Cargo.lock`, your
  crate's `Cargo.toml`, and the registration points named in decision 0011 (one module line,
  one enum variant, one match arm).
- **Frozen contracts:** `crates/acs-core` public types and `gate.rs`, `spec/`, and
  `DEFAULT_SEALED` are orchestrator-only. If you need a change, do not make it: describe it in
  your report as a contract change request, and work around it if you can.
- Dependency order: a crate may only depend on crates listed before it in plan section 5
  (`acs-core, acs-policy, acs-paths, acs-shell, acs-semantic, acs-store, acs-git, acs-lore,
  acs-adapters, acs-mcp, acs-cli`).

## Definition of done

- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`,
  `cargo test --workspace --locked` and `cargo deny check` (if `cargo-deny` is installed;
  otherwise say so) all pass.
- Public items are documented. No `unwrap()`/`expect()` outside tests.
- Tests are hermetic (temp dirs, `ACS_HOME` pointed at a temp dir, no network, no reliance on the
  user's git config — set `user.email`/`user.name` in test repos) and cross-platform, with `cfg`
  gates for OS-specific checks. For Windows-specific code, type-check with
  `rustup target add x86_64-pc-windows-gnu` and
  `cargo clippy -p <crate> --tests --target x86_64-pc-windows-gnu -- -D warnings` where the crate
  has no C dependencies.
- Your WP's acceptance checks from the plan are met, or the report says exactly which are not
  and why.
- A decision record `docs/decisions/<number>-<title>.md` (number given in your brief) with
  Context / Decision / Consequences, listing every choice you made beyond the plan and spec.
- Third-party formats (harness hooks, plugins, MCP): read the current official docs first and
  record the exact shapes you relied on, with links and the date, in `docs/adapters/<harness>.md`.
  Capture real or documented payloads as fixtures under `tests/fixtures/<harness>/`.

## Commits

- Work on the branch of the worktree you were given. Commit as you go with clear messages and
  the attribution trailer your harness asks for.
- Never push, never open PRs, never merge into other branches, never rewrite history that
  existed before you started.

## Resources

- 4 CPUs are shared with other subagents. Run cargo with `CARGO_BUILD_JOBS=2` (your brief may
  say otherwise). Avoid `--release` builds except for performance measurements.
- Do not run `cargo clean` on the shared registry or touch other worktrees.

## Report (your final message)

1. Commits (hash and subject).
2. Public API you added, with signatures.
3. Acceptance evidence with numbers (test names, timings).
4. Decisions made (short; the record has the detail).
5. Contract change requests, if any.
6. What is untested and on which platforms.
7. Notes for later WPs.
