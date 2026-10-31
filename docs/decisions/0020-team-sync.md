# 0020 — Team sync: refs, ledger, detached `acs sync` (WP3.1)

Date: 2026-10-31. Status: accepted (orchestrator).

## Context

WP3.1 shares claims and the ledger through the repository's own git remote (plan section 4.4,
SPEC section 8). The plan fixes the ref layout and asks for all ref storage behind one interface,
so that a forge rejecting custom refs costs one more implementation. This record lists the
choices made beyond the plan and spec, and the results of the checks the plan asks to record.

## Decision

### Storage

- `acs_git::refs::SyncStorage` speaks of pilots and documents, never of refs. `CustomRefs`
  implements it; a test runs a whole sync against an in-memory implementation, which is what shows
  the fallback branch would need nothing above the trait.
- `acs-git` now depends on `acs-core` and `acs-store`. `sync::run` needs the state database and the
  dependency order of plan section 5 allows it.
- The ledger tree holds `record.json`. A claims commit has no parent, so old claims are unreachable
  and the remote does not grow with them.

### One sync

- **Order: fetch, rebuild the view, publish, push.** The plan and the old SPEC text said push first.
  One pilot on two machines shares one claims ref, so a machine has to republish what the other
  wrote, and it can only do that after fetching. SPEC section 8 now gives the order.
- **The view is fresh once the fetch worked.** A failure after that, such as a pilot with no push
  access to `origin`, is recorded in `last_error` without making just-fetched claims stale.
- **Claims of seats on this machine are never in the view.** A copy fetched back would outlive its
  release and block the seat next to it.
- **A heartbeat is published only after moving a quarter of the claim's TTL.** Heartbeats move on
  every tool call, so publishing each would mean a push per interval per active pilot, which is the
  rate-limit risk of plan section 9.2. With the defaults this is a push about every 3 minutes
  instead of every 30 seconds, and a published claim still has 11 minutes of life. The set of
  claims, a scope, a target branch or a TTL changing always publishes.
- **A fetched document is checked against its ref**: it must parse, and it and its claims must name
  the pilot the ref belongs to, because agents are told the holder's name. A bad document gets a
  warning and that pilot's last good claims stay until they expire.
- **Diverged ledgers are joined by a commit with an empty tree** and both tips as parents. Rewriting
  would drop signatures, and the push has to stay a fast-forward. Readers skip these commits.
- **A record that cannot be signed is written unsigned.** It is worth more than its signature, and
  `acs verify` (WP4.4) will report it.
- Remote: `origin`, else the first by name. No config key: `config.toml` is a frozen contract and
  nothing has needed one yet.

### Running git

Sync must not disturb the user or wait for them, so every network call:

- passes `--no-verify` to push, because a pre-push hook that runs a test suite must not run every
  thirty seconds;
- passes `--no-write-fetch-head` (git 2.29 or newer), because `FETCH_HEAD` is the user's;
- sets `GIT_TERMINAL_PROMPT=0` and `GCM_INTERACTIVE=never`, and runs under a limit of 120 seconds,
  because git has no timeout of its own and a stuck sync would hold the lock for good. The cost is
  that `acs sync` fails, rather than asks, when credentials are not cached.

### Starting sync

- **Hooks sync only a repository with `.acs/policy/`.** A plugin enabled for a user runs in every
  repository they open, and sync publishes paths and an email address. Without adoption, only a
  person typing `acs sync` publishes anything.
- Hooks start it after a tool call and on session start when the interval has passed, and on session
  end whatever the interval, since no further event may come and the release would otherwise take a
  whole TTL to reach other machines. `acs claim` and `acs release` also start it, for the same
  reason. The ones that ignore the interval use `--wait`, so they queue behind a sync that may have
  read the claims too early.
- The lock is `pending/sync.lock`, held with `File::lock`. The OS releases it however the process
  ends, so a crash leaves no stale lock to detect.

### `acs-os`: one exception to `unsafe_code = forbid`

Decision 0001 forbids `unsafe` in every crate. This narrows that: a new crate, `acs-os`, has
`unsafe_code = deny` and one unsafe block, and every other crate keeps `forbid`.

The reason is a Windows bug found by a test, which took the hook 11.8 seconds instead of
milliseconds. `CreateProcess` hands every inheritable handle to every child, and a process's own
standard handles are inheritable, so `Stdio::null()` does not stop the detached child from holding
the hook's stdout pipe. A harness reading that pipe to its end then waits for the whole sync.
Linux hides this, because descriptors close on `exec`.

Stable Rust has no way out: `CommandExt::inherit_handles` and the process attribute list are both
unstable. `cmd /c start` leaks the same handles, and going through PowerShell adds a slow process to
a hook. Clearing `HANDLE_FLAG_INHERIT` on the three standard handles with `SetHandleInformation`
right before the spawn is one call, and with it the same test passes. `windows-sys` was already in
the lockfile through other dependencies.

Remove `acs-os` once `inherit_handles` is stable.

### Claims taken on two machines inside one sync window

There is no global atomicity without a server, so both seats can be granted one file. As first
built, each was then denied once it had fetched the other's claim, and neither could edit the file
until one let go. That is safe, but it is a standstill.

The first claim now keeps the file: the earlier `acquired_at`, then the lower `claim_id`. This
amends the claim rules (SPEC section 6, `gate::blocking_claims`, which the engine and the store both
call; `claim_conflicts` is unchanged). Choices worth recording:

- Only those two fields are compared, because both machines hold them and nothing else is shared.
  Clock skew can change who wins, but the two machines always agree.
- It only applies to a claim the seat already holds, and only against a remote claim. A seat holding
  nothing is denied as before, and one machine's claims are atomic, so they never need it.
- The later claim is not revoked. Its seat is denied until the first claim ends and then holds the
  file, with no new step and no notice.
- The check at commit time applies the same rule (`gate::gives_way`). Otherwise the seat the gate
  lets go on would be refused at `git commit`.

## Checks

**GitHub accepts `refs/acs/*`** (2026-10-31, `github.com/acs-hq/acs`, git 2.45.2 over HTTPS, a
synthetic pilot id, removed afterwards). Passed: creating a claims ref, force-updating it to an
unrelated commit, creating a ledger ref, fast-forwarding it, rejection of a non-fast-forward without
force, fetching with the ACS refspec and reading the blob, and deleting both refs. Rate limits were
not measured. The GitLab check stays deferred to the hard stop at WP9.2 (decision 0019).

**Acceptance.** `crates/acs-cli/tests/sync.rs`: a bare remote and three clones with separate
`ACS_HOME`s. A claim made in one clone denies the edit in another, naming the holder, with sync
started only by hooks.

## Consequences

- Edits made inside one sync window, before either side has fetched, are not prevented: the view
  is only as fresh as the last sync. The git backstop catches them at commit time.
- `acs doctor` does not check sync yet.
- Two machines force-pushing one pilot's claims at the same moment can lose one update until that
  machine's next sync republishes it.
- The workspace now needs git 2.29 or newer for sync.
