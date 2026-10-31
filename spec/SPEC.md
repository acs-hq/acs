# ACS Specification, version 0.1 (draft)

**Status: draft 0.1.** Everything in this document may still change before 1.0. The spec version string for this draft is `acs/0.1`.

The Agent Control System (ACS) coordinates several coding agents and humans working in one git repository. A harness (Claude Code, Antigravity, ...) calls ACS on every tool call; ACS decides, deterministically and without calling an LLM, whether the action may proceed. This document specifies the data that crosses the boundaries between harness adapters, the decision engine, the local state, the policy files committed to a repository, and the refs used to share claims and the ledger through the git remote.

The key words MUST, MUST NOT, SHOULD, SHOULD NOT and MAY are used as in RFC 2119.

## 1. Overview

### 1.1 Documents and schemas

Every wire document has a JSON Schema (draft 2020-12) in [`schemas/`](schemas/), generated from the reference implementation's types and checked in. The schemas are the authority for field names, types, required fields and value patterns; this document describes semantics. Worked examples are in [`examples/`](examples/); every example validates against its schema.

| Document | Schema | Example | Where it lives |
| --- | --- | --- | --- |
| `ToolEvent` | [`tool_event.schema.json`](schemas/tool_event.schema.json) | [`tool_event.pre_tool.json`](examples/tool_event.pre_tool.json), [`tool_event.post_tool.json`](examples/tool_event.post_tool.json), [`tool_event.session_start.json`](examples/tool_event.session_start.json) | adapter to engine (stdin of `acs hook --canonical`) |
| `Decision` | [`decision.schema.json`](schemas/decision.schema.json) | [`decision.deny.json`](examples/decision.deny.json), [`decision.allow.json`](examples/decision.allow.json) | engine to adapter (stdout) |
| `Seat` | [`seat.schema.json`](schemas/seat.schema.json) | [`seat.json`](examples/seat.json) | local state; embedded in `SessionRecord` |
| `Claim` | [`claim.schema.json`](schemas/claim.schema.json) | [`claim.file.json`](examples/claim.file.json), [`claim.symbol.json`](examples/claim.symbol.json) | local state; inside `ClaimsDocument` |
| `SessionRecord` | [`session_record.schema.json`](schemas/session_record.schema.json) | [`session_record.json`](examples/session_record.json) | the ledger (`refs/acs/ledger/<pilot-id>`) |
| `Notice` | [`notice.schema.json`](schemas/notice.schema.json) | [`notice.json`](examples/notice.json) | local state |
| `ClaimsDocument` | [`claims_document.schema.json`](schemas/claims_document.schema.json) | [`claims_document.json`](examples/claims_document.json) | `claims.json` in `refs/acs/claims/<pilot-id>` |
| `Config` | [`config.schema.json`](schemas/config.schema.json) | [`config.toml`](examples/config.toml) | `.acs/policy/config.toml` |
| `Guards` | [`guards.schema.json`](schemas/guards.schema.json) | [`guards.toml`](examples/guards.toml) | `.acs/policy/guards.toml` |
| `Territory` | [`territory.schema.json`](schemas/territory.schema.json) | [`territory.toml`](examples/territory.toml) | `.acs/policy/territory.toml` |
| `LoreFrontMatter` | [`lore_front_matter.schema.json`](schemas/lore_front_matter.schema.json) | [`lore.md`](examples/lore.md) | front matter of `.acs/policy/lore/**/*.md` |

TOML documents (policy files and lore front matter) are validated against the same schemas after conversion to JSON; TOML integers map to JSON integers and TOML arrays and tables to arrays and objects.

### 1.2 Versioning

The `spec` field of a document names the spec version it was written against. This draft defines one value, `"acs/0.1"`.

- A reader MUST reject a document whose `spec` is not a version it supports, with an error that names the value found and the versions supported.
- `spec` is required in `ToolEvent`, `SessionRecord`, `ClaimsDocument` and `Config`, and optional in `Guards` and `Territory` (where, if present, it is checked the same way). `Decision`, `Seat`, `Claim` and `Notice` carry no `spec` field; they travel inside or alongside documents that do.
- Until 1.0 a new draft MAY change anything. From 1.0, changes that add optional fields or new documents are minor revisions; any other change is a major revision and changes the version string.

### 1.3 Conventions

- **JSON.** Field names and enum values are `snake_case`. Optional fields may be absent or `null`; producers SHOULD omit them. Arrays that are semantically "none" SHOULD be omitted or empty.
- **Unknown fields.** Wire documents that adapters and engines exchange (`ToolEvent`, `Decision`) are forward-compatible: readers MUST ignore unknown fields. Policy documents (`Config`, `Guards`, `Territory`, `LoreFrontMatter`) are authored by hand; readers MUST reject unknown fields so that typos are caught, and the error SHOULD name the field. Stored records are exact: readers MUST reject unknown fields in a claim scope and MAY ignore unknown fields elsewhere.
- **Timestamps.** RFC 3339 date-times in UTC. Producers MUST write the `Z` suffix (for example `2026-10-29T14:05:11Z`); fractional seconds are allowed. Readers SHOULD also accept numeric offsets and convert to UTC.
- **Identifiers.** Opaque strings, a fixed prefix followed by 1 to 64 characters from `[0-9a-z_-]`:

  | Kind | Prefix | Meaning |
  | --- | --- | --- |
  | seat | `seat_` | one harness session in a repository |
  | session | `ses_` | one session record; the value of the `ACS-Session` trailer |
  | claim | `clm_` | one claim |
  | notice | `ntc_` | one notice |
  | lore | `lore_` | one lore entry |

  How identifiers are generated is up to the implementation; readers MUST validate prefix and character set and MUST NOT interpret the rest.
- **Repository paths** (in claims, `files_changed`): repository-relative, forward slashes, never empty, not absolute. A path is invalid if it is empty, starts with `/` or `\`, or starts with a drive letter such as `C:`. Producers MUST normalize before storing: no `.` or `..` segments, symlinks and junctions resolved, case folded where the filesystem is case-insensitive. By contrast, `ToolEvent.paths` are raw strings exactly as the harness gave them (absolute, or relative to `cwd`); normalization is a pipeline step (section 9).
- **Symbol paths.** The path of a symbol inside its file, segments joined by `.` (`RefundService.apply`). Every segment is non-empty. `A` *contains* `B` when `A`'s segments are a prefix of `B`'s; a symbol contains itself.
- **Content hashes.** The lowercase hex SHA-256 of a file's bytes, 64 characters.
- **Pilot.** The human responsible for a seat or claim, identified by git email address.

## 2. Canonical event: `ToolEvent`

Adapters translate each harness payload into a `ToolEvent`. Schema: [`tool_event.schema.json`](schemas/tool_event.schema.json).

| Field | Required | Meaning |
| --- | --- | --- |
| `spec` | yes | `"acs/0.1"` |
| `harness` | yes | Free-form harness name (`claude-code`, `antigravity`, ...). `canonical` marks synthetic events. New harnesses need no schema change. |
| `harness_session_id` | yes | The harness's own session identifier. One harness session is one seat. |
| `phase` | yes | `pre_tool`, `post_tool`, `session_start`, `prompt_submit` or `session_end` |
| `cwd` | yes | Absolute working directory when the event was raised |
| `tool` | for `pre_tool`, `post_tool` | `{name, kind}`: the harness's tool name and its classification |
| `paths` | no | Raw path strings the tool touches; for `move`, source then destination |
| `edit` | no | `{old_text, new_text}` for edit tools; enables symbol-level claims |
| `command` | for `shell` | The shell command text |
| `result` | `post_tool` only | `{success, exit_code?}` |
| `prompt` | `prompt_submit` only | The user's prompt. MUST NOT be persisted unless `ledger.store_prompts` is true. |
| `raw` | no | The original harness payload, for debugging. MUST NOT influence decisions. |

`tool.kind` is one of `read`, `write`, `edit`, `delete`, `move`, `shell`, `other`. Adapters classify conservatively: a tool that may change files and cannot be classified is `shell` or `other`, never `read`.

Session phases: `session_start` creates the seat and may return a short status as context; `prompt_submit` may return pending notices as context; `session_end` releases the seat's claims and finalizes its session record.

## 3. Decision and reason codes

The engine returns a `Decision` for every event. Schema: [`decision.schema.json`](schemas/decision.schema.json).

| Field | Meaning |
| --- | --- |
| `outcome` | `allow`, `deny`, `ask` or `queue` |
| `reason_code` | A stable machine code from the table below |
| `message` | Guidance for the agent. May be empty when nothing is blocked. |
| `claims_acquired` | Claims acquired while deciding this event (default empty) |
| `context` | Lore and notices to inject into the agent's context (default empty) |
| `warnings` | One-line warnings that do not change the outcome (default empty); adapters render them as context |

### 3.1 Outcomes

- `allow`: the harness proceeds.
- `deny`: the harness blocks the call and shows `message` to the agent.
- `ask`: the harness asks the human to approve in its own prompt. A harness that cannot prompt MUST treat `ask` as `deny`.
- `queue`: **reserved** for deferred edits, which a later draft will define. Implementations MUST NOT emit it yet. A consumer that receives it MUST treat it as `deny`.

### 3.2 Reason codes

The set is closed; adding a code is a spec change. The "outcome" column lists what implementations return with the code.

| Code | Outcome | Meaning |
| --- | --- | --- |
| `ok` | allow | Nothing blocked. |
| `gate_off` | allow | `acs off` is active for this repository on this machine. Sealed paths are still enforced. |
| `sealed_path` | deny | A target matches a sealed glob (`guards.toml`) or lies in a `sealed` zone (`territory.toml`). |
| `banned_command` | deny | A shell command matches a `destructive.ban` pattern. |
| `protected_branch` | deny | A history rewrite or force push targets a protected branch. |
| `path_escapes_repo` | ask | A write, delete or move target resolves outside the repository root, including through a symlink or junction. |
| `delete_outside_claims` | deny | Deletes exceed `destructive.max_files_deleted_outside_claims`. |
| `destructive_budget` | ask | Deletes in one command exceed `destructive.max_files_deleted_per_command`. |
| `destructive_unknown_targets` | ask | A destructive shell command whose targets cannot be determined. |
| `claim_conflict` | deny | An overlapping live claim is held by another seat (section 6). |
| `owned_territory` | deny | A non-owner agent touched an `owned` zone. Implementations that do not enforce ownership treat `owned` as `shared` and never return this code. |
| `state_unavailable` | deny, or allow with a warning | The state database is locked, corrupt or missing (section 11). |
| `unknown_payload` | allow | The adapter could not parse the harness payload (section 11). |
| `claim_queued` | queue | Reserved with `queue`. MUST NOT be returned yet. |

An invalid policy file that falls back to the last valid policy, and a stale remote claims view, are **warnings**, not reason codes: they do not change the outcome.

### 3.3 Messages

`message` is written for the agent, not for a log. It MUST say what is blocked, by whom or by which rule, and what to do next. For example:

> src/payments/refund.ts is claimed by sara@example.com (Claude Code, active 4 min). Work on other files, or ask your pilot to override.

Every reason code has a fixture in the conformance suite (planned for spec 1.0).

## 4. Core records

### 4.1 `Seat`

One agent session or human editing session in a repository; exactly one per harness session. Fields: `seat_id`, `pilot`, `harness`, `harness_session_id`, `worktree` (absolute path), `branch` (absent when HEAD is detached), `target_branch`, `started_at`, `last_seen_at`. A seat is created on the first event of a session.

### 4.2 `Claim`

A seat's exclusive right to change a scope until released or expired. Fields: `claim_id`, `seat_id`, `pilot`, `scope`, `target_branch`, `acquired_at`, `heartbeat_at`, `ttl_secs`, `origin` (`agent` or `human`).

`scope` is exactly one of:

```json
{ "file": "src/payments/refund.ts" }
{ "symbol": "src/payments/refund.ts", "symbol_path": "RefundService.apply" }
```

A claim is **expired** at time `now` when `now - heartbeat_at >= ttl_secs`. Expiry is evaluated lazily by readers; nothing needs to delete an expired claim for it to stop blocking.

### 4.3 `SessionRecord`

The ledger entry for one session. `session_id` is the value of the `ACS-Session` trailer on the session's commits. Fields: `spec`, `session_id`, `seat` (the full `Seat`), `intent` (optional text the agent declared through the `acs_intent` MCP tool), `claims` (history: `claim_id`, `scope`, `acquired_at`, optional `released_at` and `release_reason` of `released`, `expired` or `session_end`), `files_changed` (`path`, `pre_hash`, `post_hash`; `null` means the file did not exist before or no longer exists), `guard_decisions` (`counts` per reason code and a list of `notable` decisions with `at`, `reason_code`, `outcome`, `path_or_command`), `commits` (full commit hashes carrying the trailer), `lore_loaded` (lore ids), `started_at`, `ended_at`, `ended_reason` (`clean` or `crashed`), `acs_version`.

A record for a session that ended without an end event is assembled on a later run with `ended_reason: crashed` and `ended_at` set to the seat's `last_seen_at`.

Records are pushed to the git remote. They MUST contain only structural data (paths, hashes, counts, ids, the agent's own intent text); never prompts unless `ledger.store_prompts` is true, never file contents. `path_or_command` holds a path or a command line and therefore MUST be redacted of secrets and SHOULD be truncated.

### 4.4 `Notice`

A one-way system message from ACS to a seat, delivered once in the seat's next hook context. Fields: `notice_id`, `kind` (`interface_changed`, `claim_released`, `queued_edit_applied`, `lore_stale`), `target_seat`, `body` (written for the agent), `created_at`, `delivered_at` (absent until delivered). Notices have no reply mechanism; ACS has no agent-to-agent messaging.

### 4.5 `ClaimsDocument`

The `claims.json` blob published in `refs/acs/claims/<pilot-id>` (section 8): `spec`, `pilot`, `updated_at`, `claims` (that pilot's live claims across all their seats).

## 5. Policy files

Policy is committed under `.acs/policy/` and is the same for every clone. All policy files are optional except as noted; a missing file means built-in defaults.

```
.acs/policy/
  config.toml
  guards.toml
  territory.toml
  lore/            mirrors the source tree, for example lore/payments/money.md
```

All four types reject unknown fields. **Glob dialect.** Path globs (`sealed`, zone `paths`, lore `scope`) are matched against repository-relative, `/`-separated paths and are anchored at the repository root. `*` and `?` match within one path segment and never match `/`; `**` as a whole segment matches zero or more segments; `[abc]` and `{a,b}` are supported; there is no negation. A pattern that should match at any depth MUST say so (`**/.env*`). Matching is case-insensitive on case-insensitive filesystems and case-sensitive otherwise. `protected_branches` uses the same dialect against branch names (so `release/*` does not match `release/a/b`). `destructive.ban` patterns are matched against each simple shell command, its words joined by single spaces, where `*` matches any run of characters including `/` and spaces.

### 5.1 `config.toml`

Only `spec` is required, so a file containing just `spec = "acs/0.1"` is valid.

| Key | Default | Meaning |
| --- | --- | --- |
| `spec` | required | `"acs/0.1"` |
| `default_target_branch` | `"main"` | Integration branch when a seat's own target is unknown |
| `claim_ttl_secs` | `900` | Seconds a claim lives without a heartbeat; at least 1 |
| `sync_interval_secs` | `30` | Seconds between background syncs; at least 1 |
| `[ledger] store_prompts` | `false` | Whether prompts may be stored in the ledger |

### 5.2 `guards.toml`

Every key defaults to empty (zero for the numeric limits noted); the built-in default guards are applied by the policy loader when the file or key is absent. `spec` is optional.

| Key | Meaning |
| --- | --- |
| `sealed` | Globs of paths agents may never write |
| `protected_branches` | Branch globs on which history rewrites and force pushes are denied |
| `[destructive] max_files_deleted_outside_claims` | Default `0`: any delete outside the seat's claims is denied (`delete_outside_claims`) |
| `[destructive] max_files_deleted_per_command` | Default `50`: a command deleting more files asks the human (`destructive_budget`) |
| `[destructive] ban` | Command patterns that are always denied; `*` matches any run of characters (`banned_command`) |

**Default sealed globs.** The default `sealed` list is below. It exists so that an agent cannot switch ACS off by editing its own configuration. Adapters extend it with their harness's hook and settings files.

```
.git/**
.acs/**
.github/workflows/**
**/.env*
**/.claude/settings*.json
**/.claude/hooks/**
**/.agents/**/hooks*.json
**/.agent/**/hooks*.json
**/.agents/plugins/**
**/.agent/plugins/**
**/.agents/plugins.json
**/.agent/plugins.json
.husky/**
```

### 5.3 `territory.toml`

Zones say who may change what. Every key defaults to empty, which makes the whole repository `shared`. `spec` is optional.

```toml
[[zone]]
paths = ["payments/**"]
kind = "owned"
owners = ["sara@example.com"]

[teams]
platform = ["omar@example.com", "lina@example.com"]
```

| Zone `kind` | Effect on agents |
| --- | --- |
| `owned` | Only the owners' agents may change it (`owned_territory` otherwise); a human override lifts this |
| `shared` | A claim is required |
| `open` | No claim is required |
| `sealed` | Never writable (`sealed_path`) |

When zones overlap, the most specific match wins. `owners` entries are a git email address or `team:<name>`, where `<name>` is a key of `[teams]`; a `team:` owner naming no team is a policy error reported by the loader.

### 5.4 Lore

A lore entry is a Markdown file under `.acs/policy/lore/`, laid out to mirror the source tree (`lore/payments/money.md`). It starts with TOML front matter between two lines consisting of `+++`; the rest is the body, injected into an agent's context.

```markdown
+++
id = "lore_7f3a"
scope = ["payments/**"]
owner = "sara@example.com"
written_against = "a1b2c3d"
+++
Amounts are integer cents. Never call Stripe directly; go through `PaymentGateway`.
```

| Key | Meaning |
| --- | --- |
| `id` | Stable `lore_` identifier |
| `scope` | Globs of the paths the entry applies to |
| `owner` | Email or `team:<name>`; reviews proposed changes |
| `written_against` | Git revision the entry was written against; staleness is measured from it |

The text must start with a `+++` line (an optional byte-order mark is tolerated); the front matter ends at the next `+++` line; trailing whitespace on a fence line is ignored; the body is everything after the closing fence line, verbatim. Agents never write lore directly: `.acs/**` is sealed.

## 6. Claim rules

A requested claim **conflicts** with an existing claim when **all** of the following hold:

1. **The scopes overlap.** Same file; or, for two symbol claims in the same file, the same symbol or one containing the other (section 1.3); a file claim overlaps every symbol claim in that file.
2. **The holders are different seats.** Two seats of the same pilot are different holders: one person running three agents is the main use case.
3. **Both have the same `target_branch`.** A throwaway experiment branch never blocks work targeting another branch.
4. **The existing claim has not expired** (section 4.2), or it comes from a remote view that is not stale (section 8).

A conflict yields `deny` with `claim_conflict`; the message names the holder, their harness, how long the claim has been active, and what to do instead. Acquiring all claims an event needs MUST be atomic: either all are granted or none.

**Simultaneous claims.** With no server, two machines can each grant a claim on one scope before either has fetched the other's. A seat that holds a live local claim on the scope is then not blocked by a conflicting remote claim that comes after its own: the earlier `acquired_at` comes first, then the lower `claim_id` in byte order. Implementations MUST compare nothing else, so that both machines reach the same answer; a difference between their clocks can change which claim comes first, never whether they agree. The later claim is not revoked: its seat is denied until the earlier claim ends, and then holds the scope. A third seat is blocked by both. The same rule applies to the check at commit time. Claims on one machine never need it, because they are acquired atomically.

Territory is checked before claims: a `sealed` zone always denies agents; an `owned` zone denies non-owner agents unless a human override exists; a `shared` zone requires a claim; an `open` zone needs none.

## 7. Commit trailers

Commits are linked to sessions with a git trailer `ACS-Session: <session_id>` (a `ses_` id), added by the `prepare-commit-msg` git hook.

- When an agent's shell tool runs `git commit`, the pre-tool step records a pending-commit marker for that worktree and session; the git hook consumes it.
- For a commit made by a human, one trailer is added per session whose attributed changes are in the staged diff.
- A commit may carry several trailers.

## 8. Refs and sync

Claims and the ledger are shared through the repository's own git remote; no other server is involved.

| Ref | Contents | Writer | Update rule |
| --- | --- | --- | --- |
| `refs/acs/claims/<pilot-id>` | One commit whose tree holds `claims.json` (a `ClaimsDocument`) | That pilot only | Force-updated on change; each pilot owns one ref, so pushes never conflict |
| `refs/acs/ledger/<pilot-id>` | Append-only commit chain, one commit per `SessionRecord` | That pilot only | Fast-forward only; commits are signed when git signing is configured |

**Pilot id.** `pilot-id` is the first 16 lowercase hex characters of the SHA-256 of the pilot's git email, trimmed and lowercased. Ref names therefore never contain raw email addresses.

| Email | `pilot-id` |
| --- | --- |
| `sara@example.com` | `259b65833bbadfd5` |
| `omar@example.com` | `0de61b1110462435` |

(`Sara@Example.com` and ` sara@example.com ` give the same id as `sara@example.com`.)

**Fetch refspec.** `acs init` adds `+refs/acs/*:refs/remotes/<remote>/acs/*` so that other pilots' refs never overwrite your own namespace.

**Commits.** A claims commit has no parent and its tree holds only `claims.json`. A ledger commit's tree holds only `record.json` (one `SessionRecord`), and its parent is the previous ledger commit. When one pilot works on two machines their chains diverge; the chains are joined by a commit with both tips as parents and an empty tree, because a ledger is never rewritten. Readers skip commits with no `record.json`.

**Sync.** `acs sync` runs against one remote (`origin`, else the first remote by name), in this order:

1. Fetch all of `refs/acs/*`.
2. Rebuild the remote-claims view from the fetched documents. Claims of seats on this machine are left out: the local state is their truth. A document that does not parse, or names a pilot other than the owner of its ref, is ignored with a warning and that pilot's last good claims stay until they expire. A pilot whose claims ref is gone from the remote leaves the view.
3. Build each local pilot's document: their live claims on this machine, plus the unexpired claims in their fetched document from seats not on this machine, because one pilot on two machines shares one ref. Publish it only if something others need to know has changed: the set of claims, a scope, a target branch, a TTL, or a heartbeat that has moved by at least a quarter of the claim's TTL.
4. Join each local ledger with its fetched copy, so that the push is a fast-forward.
5. Push what changed. A sync with nothing new does not push.

The view is **stale** when the last successful fetch is strictly older than `3 x sync_interval_secs`, or there never was one. A failure after step 1, such as a push the remote rejects, is reported but does not make the view stale. A hash chain comes from git commit parents; signing uses the user's git signing configuration, and a record that cannot be signed is written unsigned rather than lost.

Sync runs detached and never blocks an agent. Hooks start it only in a repository with a remote and a `.acs/policy/` directory, because it publishes paths and the pilot's email: when the interval has elapsed since the last attempt, on session start and after a tool call; and on session end, whatever the interval, because the claims just released would otherwise block other machines until they expire. At most one sync runs per repository on a machine. Sync never prompts for credentials and never runs the user's pre-push hooks. With no remote, ACS works in local-only mode.

## 9. Evaluation order

For every event the engine proceeds as follows. A step that denies ends evaluation.

**Pre-tool**

1. The adapter parses the harness payload into a `ToolEvent` (failure: `unknown_payload`).
2. Resolve the repository and the seat (created on the first event of a session).
3. Normalize every path: absolute, canonical, symlinks and junctions resolved, case-folded where the filesystem is case-insensitive. Flag any path that escapes the repository root (`path_escapes_repo`).
4. Guards: sealed paths, banned commands, protected-branch rules, destructive budgets (`sealed_path`, `banned_command`, `protected_branch`, `delete_outside_claims`, `destructive_budget`).
5. Territory and claims: compute the required claims and acquire them atomically (`owned_territory`, `claim_conflict`).
6. Snapshot the pre-images of files about to change. For a destructive shell command whose targets cannot be determined, ask (`destructive_unknown_targets`).
7. Collect context to inject: lore for scopes entered for the first time in the session, and pending notices.
8. Render the `Decision` back through the adapter.

**Post-tool.** Determine what actually changed (the event's paths, or a working-tree diff for shell commands), attribute it to the session, store post-images, update heartbeats, and spawn a detached sync when the interval has elapsed.

**Precedence.** Steps 3–6 are all evaluated; their results combine as deny > ask > allow. If any step denies, the outcome is `deny` with the `reason_code` of the first denying step in the order above. Otherwise, if any step asks, the outcome is `ask` with the first asking step's code. A `deny` acquires no claims and takes no snapshots; on `ask`, claims are acquired and pre-images snapshotted as for `allow` (if the human declines, the claims simply expire or are released at session end). Warnings from every step are collected into `warnings` regardless of outcome.

## 10. Local state layout

State is per machine, not per clone: all worktrees and clones of a repository share it. It lives under `ACS_HOME` (default `~/.acs`, overridden by the `ACS_HOME` environment variable), never inside the working tree, so an agent deleting files in the tree or in `.git` cannot reach snapshots.

```
ACS_HOME/
  index/<key>.json  identity cache: canonical common git dir -> repo-id
  repos/<repo-id>/
    state.db        SQLite: seats, claims, events, notices, stats, remote-claims view
    snapshots/      content-addressed blobs
    cache/          compiled policy, keyed by the hash of .acs/policy/
    pending/        pending-commit markers and proposed lore
```

**Repository id.** `repo-id` is the SHA-256 (lowercase hex) of the repository's root-commit hashes: the parentless commits reachable from `refs/heads/*` and `refs/remotes/*`, excluding `refs/remotes/*/acs/*` (ACS's own ledger and claims chains are unrelated histories and MUST NOT affect the id). Tags, notes and replace refs are ignored. The hashes are lowercased, de-duplicated, sorted ascending and joined with `\n` (no trailing newline). The id is stable across clones and every worktree of a repository. Clones that track different branch sets of a multi-root repository can disagree; this is accepted.

- **No commits yet:** the id is SHA-256 of `path:` followed by the canonical common git directory (not the work tree), lowercased on case-insensitive platforms. When a later resolution finds commits, `repos/<path-id>/` is moved to `repos/<commit-id>/`; if the target exists, the directories are merged without overwriting anything and conflicts are left in place and reported.
- **Shallow clones:** the shallow boundary commits stand in for the roots. The resulting id is provisional and is not migrated when the clone is deepened.
- **Caching:** implementations SHOULD cache the id per common git directory outside the working tree and `.git` (`ACS_HOME/index/`), so the hot path neither spawns processes nor walks history. Once cached, an id stays fixed on that machine even if new roots appear.

**Permissions.** Directories and files under `ACS_HOME` are readable only by their owner (Unix: `0700` directories, `0600` files). On Windows they inherit the ACL of their parent, which is owner-only for the default location under the user profile.

## 11. Failure modes and privacy

| Situation | Behavior |
| --- | --- |
| Sealed path or banned command | Fail closed: deny, always |
| State database locked, corrupt or missing | Deny destructive actions; allow ordinary edits with a warning in `context` (`state_unavailable`); `acs doctor` repairs |
| Remote unreachable, claims view stale | Fail open on remote claims, with a one-line warning; local claims are still enforced |
| Policy file invalid | Fall back to the last valid compiled policy and warn; if none, built-in default guards only |
| Unknown harness payload | Allow, log the raw payload, show a warning once per session (`unknown_payload`) |
| `acs off` | Gate disabled for this repository on this machine, except sealed paths (`gate_off`); visible in `acs status` |

**Privacy.** Prompts are never stored unless `ledger.store_prompts = true`. Intent comes from the agent calling the `acs_intent` MCP tool, which the usage skill tells it to do at the start of a task. Ledger records are pushed to the remote, so they MUST contain nothing that is not already safe to push.

## 12. Not yet specified

These are known gaps in this draft: the conformance suite and its runner (planned for 1.0); the adapter support tiers; the exact message templates per reason code; and the behavior of `queue`.
