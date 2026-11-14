# 0042 — CI workflows are not sealed by default; example env files are writable

Date: 2026-11-14. Status: accepted. Amends decision 0017 and settles an open item of decision
0031 (issue 62). Decided by: the human for what changes (items 1 and 2); Claude for how the
`.env` change is built (item 3) and the rest (items 4 to 6).

## Context

Two built-in sealed paths were likely to be the first friction a new user meets. "Fix my CI" is a
common thing to ask an agent, and `.github/workflows/**` made it a flat deny. `**/.env*` also
matched `.env.example` and `.env.sample`, which hold no secrets and which agents edit as part of
ordinary work. A repository cannot shrink the built-in list (`sealed` in `guards.toml` only adds),
so either change had to be a change to the defaults. The glob dialect (SPEC section 5) has no
negation.

## Decision

1. **`.github/workflows/**` leaves the built-in list.** Decided by: the human. A repository that
   wants workflows sealed adds the glob to `sealed` in `.acs/policy/guards.toml`:

   ```toml
   sealed = [".github/workflows/**"]
   ```

2. **`**/.env*` no longer covers example files.** Decided by: the human. Real env files (`.env`,
   `.env.local`, `.env.production`, `.env.development.local`, `.envrc`, ...) stay sealed.
3. **How: a built-in exception, not a set of narrower globs.** Decided by: Claude.
   `DEFAULT_SEALED_EXCEPTIONS` in `acs-core` pairs `**/.env*` with the exception glob
   `**/.env*.{example,sample,template,dist}`: a path that matches the exception is not sealed by
   `**/.env*` (another sealed glob can still cover it). `acs-policy`'s compiled matcher applies it
   (`Seal` in `compiled.rs`), so the hook, the MCP tools, `acs claim` and the git hooks all follow
   it; no adapter or hook keeps its own copy of the list. In one sentence for the docs: every
   `.env` file is sealed except example files, whose names end in `.example`, `.sample`,
   `.template` or `.dist`. Why not explicit globs: without negation, the only way to leave the
   examples out with globs is to list the names that stay sealed (`.env`, `.env.local`,
   `.env.*.local`, `.env.production`, ...), and every name not on the list (`.env.prod`,
   `.envrc`, `.env.backup`) would silently become writable. The exception narrows the seal by
   exactly the example files and nothing else.
   - `.template` and `.dist` are included: both are common names for committed example files
     (`.env.dist` in Symfony), and they fit the same suffix rule.
   - The rule is a suffix, so `.env.local.example` and `.env.production.sample` are examples too;
     `.env.example.local` (a local copy) is not.
   - Only the glob `**/.env*` carries the exception, and the comparison follows the policy's case
     setting. A repository that wants example files sealed adds a glob that names them, such as
     `**/.env*.example`; its own globs never carry an exception. A repository that lists
     `**/.env*` itself gets the built-in entry (duplicates are merged), exception included.
4. **What agents and people are told.** `acs status` shows the exception next to `**/.env*`, the
   session-start summary says "`**/.env*` (except `...`)", and the MCP `acs_status` reply adds a
   note that example files are writable. The JSON `sealed` lists stay plain globs.
5. **`CACHE_FORMAT` goes from 3 to 4**, so a cached policy built with the old list is not reused.
6. **Self-protection is unchanged.** Neither glob was in `SELF_PROTECTION_SEALED`, and an exception
   may never belong to one of those (a unit test checks it).

## Consequences

- An agent can edit `.github/workflows/` in every repository that does not seal it, including this
  one: its `.acs/policy/guards.toml` lists no `sealed` entries, so after the next reinstall of
  `acs` agents here can change workflows unless the human adds the glob (agents cannot edit
  `.acs/`).
- The policy template `acs init` writes says that `sealed` adds to the built-in list, names the
  `.env` exception and shows how to add workflows. Existing `guards.toml` files keep their old
  comment, which still lists `.github/workflows/**` among the defaults; it is a comment only.
- SPEC section 5.2 lists 12 default globs and states the exception; `spec/examples/guards.toml`
  follows.
- **Reinstall `acs` after merging** for the hooks to use the new defaults; no schema change.
