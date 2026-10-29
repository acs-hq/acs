# 0003: ACS_HOME and repository identity

Date: 2026-10-29 (WP0.3)

## Context

All local ACS state lives under `ACS_HOME/repos/<repo-id>/` (SPEC section 10). WP0.3 builds the resolution of `ACS_HOME`, the directory layout, owner-only permissions, and the `repo-id`: one id per repository, shared by every clone and linked worktree on a machine. `repo-id` is computed on every hook call, so the lookup is on the 15 ms budget; the first computation may walk history, which is slow on big repositories.

## Decision

**ACS_HOME.** `ACS_HOME` if set and non-empty (must be absolute, otherwise an error that says so), else `<home dir>/.acs`. The home directory comes from `std::env::home_dir` (Rust 1.97), which honors `USERPROFILE` on Windows, so no extra crate is needed. Resolution is a pure function of the two inputs (`AcsHome::resolve`), so tests never mutate the process environment. Nothing is created until `ensure()`. `AcsHome::at` takes an explicit root for tests and for callers that already resolved it.

**Layout.** `repos/<repo-id>/{state.db, snapshots/, cache/, pending/}` as in SPEC; `state.db` is a file that the state store creates, not this crate. Added `ACS_HOME/index/` for the identity cache below. Both are additions to SPEC section 10 (see "Spec follow-ups").

**Permissions.**

- Unix: every directory ACS manages is created `0o700`; a managed directory that already exists with group or other bits is tightened (`chmod`) to `0o700`, because a pre-existing loose `ACS_HOME` would otherwise silently expose snapshots. The managed set is `ACS_HOME`, `repos/`, `index/`, `repos/<id>/` and its three subdirectories. Parents above `ACS_HOME` are never touched. Files are created `0o600` (`fsutil::private_file_options`, `write_private_atomic`); since the directories are `0o700` the file mode is defence in depth. If a directory is owned by someone else, `chmod` fails and `ensure()` returns the error, which is the desired refusal.
- Windows: directories are created with default attributes and inherit the ACL of their parent. Under the default `%USERPROFILE%\.acs` that ACL grants access to the user, SYSTEM and Administrators only, which meets the "owner only" intent in practice. **Residual risk:** if `ACS_HOME` points somewhere with a permissive inherited ACL (a drive root, a shared folder), other local users may read the state, and ACS does not detect or repair this. Setting an explicit ACL needs either `unsafe` Win32 calls (`unsafe_code` is forbidden workspace-wide) or a dependency such as `windows-acl`, or spawning `icacls`; none is simple, dependency-light and testable without a Windows machine. Revisit if the threat model (WP10.3) calls for it; `acs doctor` is the natural place to warn.

**Repository identity.**

- *Rule.* `repo-id` = lowercase hex SHA-256 of the root-commit hashes, lowercased, sorted ascending, de-duplicated, joined with `\n` (no trailing newline). One root gives SHA-256 of the 40-character (or 64 for SHA-256 object format) hash.
- *Which roots.* SPEC says only "the repository's root-commit hashes". The rule used is: **the commits without parents that are reachable from `refs/heads/*` and `refs/remotes/*`, excluding `refs/remotes/*/acs/*`.** Reasons: (1) it does not depend on `HEAD`, so a linked worktree on an unrelated orphan branch (`gh-pages`) still gets the repository's id; (2) a plain `git clone` has the same branches as local or remote-tracking refs, so clones agree; (3) tags, notes, stashes and `refs/pull/*` are excluded because they are not part of the project history and vary per clone; (4) ACS's own `refs/acs/*` (and their fetched copies under `refs/remotes/<remote>/acs/*`) hold commit chains that are roots unrelated to the code and would change the id the first time anyone syncs, so they must be excluded; `--all` is therefore wrong. Replacement refs and grafts are ignored (`--no-replace-objects`), which would otherwise change parents per clone.
- *Known instability.* Clones that differ in which branches they track (`--single-branch` of a repository with several roots) get different ids. Single-root repositories, the overwhelming majority, are unaffected. Once an id is cached on a machine it does not change when new roots appear later.
- *Shallow clones.* In a shallow clone the "roots" are the shallow boundary commits, not real roots. The id is computed anyway and flagged `IdKind::Shallow` (provisional). It is cached with the `shallow` file's size and mtime, so deepening or unshallowing invalidates it. State under a provisional id is not migrated to the full id.
- *Empty repository.* The id is SHA-256 of `path:` followed by the canonical common git directory (not the work tree, so it is the same from any worktree), lowercased on Windows and macOS where the default filesystems fold case. Detection is process-free: no loose or packed branch/remote refs (reftable is assumed to have commits and git decides). Resolving creates nothing on disk for such a repository.
- *Common directory.* Found by walking up from the start path (which may not exist yet; the nearest existing ancestor is used): a `.git` directory, a `.git` file (`gitdir: <path>`, relative or absolute) whose target's `commondir` file leads to the common directory, or a bare layout (`HEAD`, `objects/`, `refs/`). Everything is canonicalized with `dunce::canonicalize`, so ids and cache keys never contain `\\?\` prefixes and macOS `/var` vs `/private/var` agree. `GIT_DIR` and friends are ignored on purpose: a call made from inside a git hook must still describe the repository that contains the start path.

**Migration on first commit.** When a resolve finds commits for a repository, it moves `repos/<path-id>/` to `repos/<commit-id>/`:

- target absent: atomic directory rename;
- target present (another clone got there first, or an earlier partial run): merge. Entries absent from the target move over; a source file that is byte-identical to the target's is dropped; **anything else stays in the old directory** and is returned in `Migration::Merged::left_behind` so the caller can warn. The target's `state.db` always wins, and the `state.db*` sidecar files (WAL, SHM) are never separated from their database. Nothing is overwritten, and nothing is deleted except exact duplicates.
- a failed move (for example a file held open on Windows) is not fatal: the commit id is returned, `migration_error()` carries the message, the result is not cached so the next resolve retries.

**Cache.** `ACS_HOME/index/<sha256(canonical common dir)>.json` holds `{version, common_dir, id, kind, roots, stamp}`. A hit needs: parseable JSON, same version, same path string, `id` equal to the hash recomputed from `roots`, and an unchanged stamp. The stamp is the git directory's creation time plus the `shallow` file's size and mtime. The creation time makes a repository deleted and re-initialized at the same path miss the cache (where the filesystem reports creation times; otherwise only that one case is undetected). The cache lives outside the working tree and `.git`, is written atomically with `0o600`, and is only an optimization: any problem reading it means recompute, any problem writing it is ignored. A cached lookup does: canonicalize, a few `stat`/`read` calls to find the common dir, one small file read, one JSON parse, one SHA-256 over the roots. No process, no history. Path-derived ids are not cached (the check is cheaper than the cache).

**git CLI versus gix for the uncached path.** Chose `git rev-list --max-parents=0` via `std::process::Command`, with `--git-dir` pointing at the common directory and the `GIT_DIR` family removed from the child's environment. Evidence (measured 2026-10-29, gix 0.90.0, `default-features = false`, features `revision` + `sha1`, the smallest set that opens a repository and walks revisions):

| | gix | git CLI |
| --- | --- | --- |
| extra crates | 128 | 0 (`thiserror`, `dunce`, `serde` only) |
| release build of a one-line program | 32 s wall, about 100 s CPU | none |
| binary size for that program | 2.7 MB | none |
| SHA-256 object format | needs the `sha256` feature, not verified | works |
| reftable, commit-graph, `extensions.*`, future formats | whatever gix implements | whatever the user's git implements |
| root walk speed | not faster: a root walk visits every commit either way | uses the commit-graph file when present |
| cost on the hot path | none (cached) | none (cached) |

The cached path is what runs per hook, and it uses neither. The uncached path runs once per repository per machine, where a 5 to 80 ms process spawn is irrelevant and compatibility with whatever format the user's repository uses matters more than avoiding a spawn. The plan's preference for `gix` stands for the reads that sit on the hot path (WP2.4/WP3.1 may still want it, for example for index stat data); this crate does not need it and is not allowed to depend on `acs-git` anyway. Revisit if the one-time walk proves too slow on very large repositories; the `RootFinder` trait makes the swap local.

Measured on this machine (Linux, release build, tmpfs-like disk): uncached resolve of this repository (18 commits) 2.8 ms including the spawn; a 20,000-commit synthetic repository 81 ms; cached resolve 14 to 19 µs at p50 and under 70 µs at p99 (200 iterations), with the process-start cost of the caller excluded.

## Consequences

- Other crates get one entry point, `RepoIdentity::resolve(&AcsHome, &Path)`, then `identity.home(&acs_home).ensure()`. `RootFinder` is the seam for tests that must prove no history is read.
- The state store (WP1.2) must open `state.db` with `private_file_options`/mode `0o600` and must treat `repos/<id>/state.db*` as one unit (the migration code already does).
- A repository whose branch set grows a new root later keeps its cached id on this machine; a second machine may compute a different id. `acs doctor` should offer a "refresh identity" action (delete the cache file; the next resolve recomputes and, if the id changed, state stays under the old id).
- Windows and macOS behavior of the creation-time stamp, `commondir` parsing with drive-letter paths, 8.3 short temp names, and case folding is covered by tests that run on all three CI runners but could not be run locally.

## Spec follow-ups (for the orchestrator)

SPEC section 10 should say which roots count ("commits without parents reachable from `refs/heads/*` and `refs/remotes/*`, excluding `refs/remotes/*/acs/*`"), that the path fallback hashes `path:` plus the canonical common git directory (lowercased on case-insensitive platforms), list `ACS_HOME/index/`, and note that shallow clones get a provisional id.
