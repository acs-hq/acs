# 0012 — Hook pipeline (WP2.1)

Date: 2026-10-30. Status: accepted (WP2.1), pending orchestrator review.

## Context

WP2.1 builds `acs hook`: the adapter interface and the canonical adapter, the local git reads and the
working-tree change detection, the store additions of decision 0011 section 4, and the pipeline in
`acs-cli` (pre-tool, post-tool, session events, pending-commit markers). Decision 0011 fixed the
crate roles and the cross-WP interfaces; this record lists every choice made beyond it, the plan and
the spec, and the measurements behind the performance choices.

## Decision

### Adapter interface (`acs-adapters`)

- `trait Adapter: Sync { name; parse(phase: Option<Phase>, payload) -> Result<ToolEvent, AdapterError>;
  render(event: Option<&ToolEvent>, decision) -> HookOutput }` as in the brief. `AdapterError`
  carries the message plus the `harness_session_id` and `cwd` recovered from the payload when it was
  a JSON object (`harness_session_id` or `session_id`; `cwd`): the pipeline needs the working
  directory to find the repository and the session id to warn once per session.
- The registry is a static slice `ADAPTERS: &[&dyn Adapter]`; `by_name` (case-insensitive) and
  `names()` read it. A harness adapter adds one `mod` line and one slice entry.
- `canonical`: a given `phase` (from `acs hook --canonical <phase>`) overrides the event's own
  `phase`, so one document can be replayed through several phases. The exit code is always 0; the
  decision is in the document.
- `acs hook` argument errors exit 1, never clap's 2: exit 2 means "block the tool call" to Claude
  Code. An unknown harness name also exits 1 with the list of adapters on stderr.

### Local git reads (`acs-git`)

- No `gix`. Everything on the hot path is a few small file reads (`HEAD`, `config`, the index
  header); `git` is spawned for what needs git's own logic. `acs-git` does not depend on
  `acs-store`: its own `discover` (the same walk as `find_common_dir`, plus the work tree root and
  the per-worktree git dir) was simpler than adapting that function.
- Pilot email: `git config --get user.email`, spawned only when a seat is created. **Fallback when
  unset:** `<$USER or $USERNAME, lowercased>@localhost` (`unknown@localhost` without either): stable
  for a person on a machine, so their seats share an identity, and obviously not a deliverable
  address.
- A remote exists when the common directory's `config` has a `[remote "..."]` section (includes are
  not followed; a remote defined only in an include leaves ACS in local-only mode).
- All spawned `git` processes run with the `GIT_DIR` family removed and `GIT_OPTIONAL_LOCKS=0`.

### Working-tree change detection (`acs-git::status`) and post-tool attribution

"Changed by this command" means modification or status-change time at or after the time the
pre-tool step recorded (a tool marker, `pending/tool/<seat>.json`; the seat's previous
`last_seen_at` when there is none), with 20 ms of slack (2 s when the filesystem reports whole
seconds). Two strategies, chosen by the index entry count from the index header (`ScanMode::Auto`):

- **Full** (up to 20,000 index entries): `git status --porcelain=v2 -z --untracked-files=all
  --no-renames`, each reported path `stat`ed and filtered by time. Exact, respects `.gitignore`.
  Tracked files found deleted are compared with the previous scan's list (`pending/scan/<key>.json`)
  to find deletions made since; git cannot say when a file disappeared.
- **Scoped** (more entries): the command's predicted targets, the seat's claimed files, and the
  entries (no recursion) of the directories where the command ran, where its targets live and where
  the seat's claims live. Unnamed candidates are filtered with `git check-ignore --no-index` (no
  index read; a force-added tracked file matching an ignore pattern is treated as ignored). **Files
  changed elsewhere by an opaque script are not seen in this mode.** This is the fallback the plan
  names in section 9.2 ("limit diffs to claimed scopes").
- Why not one approach: `git status` alone costs 110 to 290 ms on 100,000 files here (about 1.35 µs
  of `lstat` per file, mostly system time), so the 100k-file budget cannot be met with a full scan on
  this machine; below 20,000 entries it can, with headroom. `ACS_SCAN=full|scoped` and
  `ACS_SCAN_MAX_ENTRIES` override the choice.
- Paths claimed by another live seat are not attributed to this seat's command (a change in the
  same window is far more likely theirs, and a predictable write would have been denied before).
  Paths under `.git/` are never attributed.
- Nothing depends on `event.result` (Claude Code's Bash `PostToolUse` has no exit code, Antigravity's
  has no result): changes are found by looking.
- **Before-states of shell-made changes.** The hook cannot snapshot what it could not predict, so
  after the fact it records the best baseline as a pre-image: a tracked file's staged content
  (`git cat-file --batch`; the committed content in repositories above 20,000 entries, where reading
  the index costs 15 to 20 ms), absent for an untracked file born at or after the command started,
  nothing for an untracked file that existed before (unknown). This is wrong only when the file
  already had unstaged changes before the command; WP4.3 must keep that in mind and compare the
  current content with the session's post-image before restoring, as the handoff already says.
- Attribution rows (`attributions` table) are written per path per post-tool event; a repeated call
  that leaves a file as the session last left it adds nothing. `pre_hash` comes from the session's
  first pre-image (`None`: did not exist, or unknown, see above).

### Store (`acs-store`, migration 0003)

- `seats.session_id` (`ses_`, generated at creation, backfilled for old rows) and `seats.intent`;
  `SeatInfo { seat, session_id, intent, ended_at }` returned by `get_or_create_seat_info` (with a
  `created` flag), `seat_by_session`, `seat_info`, `live_seats`, `stale_live_seats` (live and
  strictly older than the threshold; **excludes the `human` pseudo-harness**, whose seats never end)
  and `set_seat_intent`. `get_or_create_seat` is unchanged for callers (it delegates).
- `last_decisions` (one row per seat), `repo_flags` (key/value, `gate_off` is `FLAG_GATE_OFF`),
  `attributions`, and `live_remote_claims` for `acs status`.

### Runtime and pipeline (`acs-cli`)

- **Seats.** An agent seat is `(harness, harness_session_id)`; the pilot is read from git only when
  the seat is created or revived. A human seat is harness `human`, session key
  `human-<16 hex of sha256(pilot "\n" worktree)>`: one per pilot and work tree. The engine always
  sees the seat's branch as `HEAD` says now (the stored one is from the first event). **Target
  branch is always `default_target_branch` from the config**; nothing in the spec says how a seat
  chooses another, so experiment branches need a later mechanism.
- **Shell and tool targets.** Tool kind `other` with paths is treated as a write (adapters must
  classify conservatively, so an unclassifiable tool that names paths may change them). A write to a
  directory is a recursive target with an empty expansion (sealed check only, so `mv x .git/`
  is caught without claiming every file of `src/` for `mv a.txt src/`); claiming a directory through
  `claim_paths` expands it (cap 500, all or nothing). Delete and move sources that are directories
  are expanded with the delete cap (the larger destructive limit plus one); globs go through
  `expand_glob`; `Partial` targets are dropped (the engine asks through `has_unknown_targets`).
  A write expansion cut at 500 files adds a warning. `mv`'s single-path form (no destination) is
  treated as a delete. Dialect from the tool name as in decision 0011.
- **Local-only repositories.** `remote_stale` is forced to `false` unless a remote exists **and** a
  sync was attempted at least once (`remote_sync_status().last_attempt_at`): before WP3.1 sets sync
  up there is no remote view to be stale, and a warning on every claim would be noise.
- **Claim races.** If `try_acquire` reports a conflict after the engine allowed, the engine decides
  again on a fresh snapshot, so the denial has the engine's own wording; only a second lost race
  falls back to a short generic message.
- **State unavailable.** Any store error while opening, reading or acquiring makes the decision run
  with `GateState::state_unavailable` (a synthetic seat `seat_unavailable` is used when no seat can
  be created); without `ACS_HOME` the policy is loaded uncached. Snapshots live in their own index
  and are still taken when `state.db` fails after the seat exists.
- **Pre-images** are captured on every pre-tool event, not only the first of a path (decision 0010:
  the first is the pre-image, later ones are cheap). A recursive request is served from the
  expansion computed for that directory; files over 32 MiB and more than 2,000 files per event are
  skipped; a snapshot failure becomes a warning, never a denial.
- **Redaction.** A command line is stored in an event's `detail` only for `deny` and `ask`, redacted
  (assignments and flags naming a secret, bearer tokens, well-known token prefixes, credentials in
  URLs) and cut to 200 characters. WP4.1 applies its own pass.
- **Unknown payloads.** Allowed with `unknown_payload`; the raw payload (first 64 KiB, newest 20) is
  saved in `pending/unknown/`; the warning is shown once per `(adapter, session)` through a
  `repo_flags` key `unknown_warned:<adapter>:<session>`, and every time when no session id could be
  recovered. Outside a git work tree the hook allows silently (`ok`).
- **Failures.** A panic inside the handler allows with a warning; the process still renders a
  decision and exits normally.
- **Session events.** `session_start` returns the status context on every start (also resume,
  clear and compaction, which drop earlier context) and revives an ended seat. `prompt_submit`
  returns context only for undelivered notices (Antigravity calls it before every model call) and
  marks them delivered first, so racing hooks of one seat never show a notice twice. `session_end`
  records, releases claims and ends the seat; no scans, no snapshot work, snapshots are kept (0011).
- **Pending-commit markers** as in 0011; `hook::markers::{worktree_key, read_commit_marker}` are
  public so WP2.4 reads them the same way.
- **Detached sync seam** in `hook::post` (`maybe_spawn_sync`): a comment marks what WP3.1 must do,
  including redirecting the child's stdin, stdout and stderr to null (an inherited pipe makes
  Claude Code wait for the hook) and not waiting on it.
- **Testing seams.** `RepoContext::open_with` and `HookEnv` take `ACS_HOME` explicitly because
  `std::env::set_var` is `unsafe`; end-to-end tests drive the built binary with `Command::env`.
- **clap** 4.6.7 with `default-features = false` and `std, derive, help, usage, error-context,
  suggestions`.

## Measurements

Linux (4 vCPUs shared with other work, virtualized filesystem), release build, process start
included, spawn to exit as seen by the test harness. `cargo test --release -p acs-cli --test perf --
--ignored --nocapture` (`ACS_PERF_FILES=<n>` sets the repository size).

| Path | Repository | Strategy | p50 | p99 | Budget p50 / p99 |
| --- | --- | --- | --- | --- | --- |
| pre-tool, file edit, warm (300 calls over 50 claimed files) | small | n/a | 8.7 ms | 18.4 ms | 15 / 40 ms: **met** |
| pre-tool, shell | 10k to 100k files | n/a | 6.0 to 7.5 ms | 9.5 to 14.3 ms | 15 / 40 ms: met |
| post-tool, shell | 10,000 files | full (auto) | 35.4 ms | 46.0 ms | 60 / 200 ms: **met** |
| post-tool, shell | 20,000 files | full (forced) | 44.9 ms | 66.3 ms | **met** |
| post-tool, shell | 100,000 files | full (forced, earlier run) | 146.9 ms | 200.9 ms | **missed**: 2.4x on p50, p99 at the limit |
| post-tool, shell | 10,000 files | scoped (forced) | 18.2 ms | 27.1 ms | met |
| post-tool, shell | 20,000 files | scoped (auto) | 15.4 ms | 25.2 ms | met |
| post-tool, shell | 100,000 files | scoped (auto) | 16.1 ms | 46.7 ms | **met, with reduced coverage** (see below) |

The post-tool shell workload: each round the command is "unpredictable", one tracked file is
modified and one new file created while it runs, and one `git cat-file` and one `git check-ignore`
run in the hook. The 100k-file figure is **only met because the scoped strategy does not look at the
whole tree**; a complete scan of 100,000 files misses the budget on this machine (`git status`
alone is 110 to 290 ms, mostly system time in `lstat`). The 20,000-entry switch point keeps full
scans under 45 ms p50 here. Earlier in development the scoped 100k figure was 49 ms p50 because
`git check-ignore` and `git cat-file` each read the 10 MB index (25 ms and 20 ms); `--no-index` and
`HEAD:` brought them to 6 and 7 ms.


## Consequences

- With the default `max_files_deleted_outside_claims = 0` an agent can delete only files it claimed
  earlier (handoff section 4); `claim_paths` is the shortcut the MCP tool will expose.
- Human claims made through `claim_paths` carry the config TTL and nothing refreshes them until
  WP7.x; a human seat never ends.
- Scoped scans (repositories above 20,000 entries) miss changes an opaque script makes outside the
  directories the command, its targets or the seat's claims touch, and full scans cost about 1.4 ms
  per 1,000 files. Both are knobs (`ACS_SCAN`, `ACS_SCAN_MAX_ENTRIES`); a later WP can add
  fsmonitor-backed scans.
- Adapter WPs need nothing from `acs-cli` beyond `acs_adapters::by_name`.
- Windows and macOS were not run: the pipeline avoids platform-specific code except the `ctime` use
  (Unix only) in `changed_since`; the hooks' temp-file and path code is exercised by the CI runners.
