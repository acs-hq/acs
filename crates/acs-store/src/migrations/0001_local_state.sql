-- Local state: seats, live claims, claim history, events, notices and counters.
--
-- Timestamps are INTEGER microseconds since the Unix epoch (UTC). Identifiers are the prefixed
-- strings of acs-core. STRICT tables make a wrongly typed write an error instead of a surprise.

CREATE TABLE seats (
    seat_id            TEXT PRIMARY KEY,
    pilot              TEXT NOT NULL,
    harness            TEXT NOT NULL,
    harness_session_id TEXT NOT NULL,
    worktree           TEXT NOT NULL,
    branch             TEXT,
    target_branch      TEXT NOT NULL,
    started_at         INTEGER NOT NULL,
    last_seen_at       INTEGER NOT NULL,
    ended_at           INTEGER,
    UNIQUE (harness, harness_session_id)
) STRICT;

-- Live claims only. A claim leaves this table (for claim_history) when it is released or found
-- expired. `symbol_path` is NULL for a file claim. `expires_at` is heartbeat_at + ttl_secs
-- (saturating), kept so that "not expired" is an indexable comparison.
CREATE TABLE claims (
    claim_id      TEXT PRIMARY KEY,
    seat_id       TEXT NOT NULL REFERENCES seats (seat_id),
    pilot         TEXT NOT NULL,
    path          TEXT NOT NULL,
    symbol_path   TEXT,
    target_branch TEXT NOT NULL,
    acquired_at   INTEGER NOT NULL,
    heartbeat_at  INTEGER NOT NULL,
    ttl_secs      INTEGER NOT NULL,
    expires_at    INTEGER NOT NULL,
    origin        TEXT NOT NULL CHECK (origin IN ('agent', 'human'))
) STRICT;

-- The hook's lookup: claims on given files within one target branch that have not expired.
CREATE INDEX claims_by_path ON claims (path, target_branch, expires_at);
CREATE INDEX claims_by_seat ON claims (seat_id);
-- A seat holds a scope at most once; try_acquire turns a repeat into a heartbeat.
CREATE UNIQUE INDEX claims_one_per_scope
    ON claims (seat_id, target_branch, path, COALESCE(symbol_path, ''));

-- Every claim that has ended, for session records.
CREATE TABLE claim_history (
    claim_id       TEXT PRIMARY KEY,
    seat_id        TEXT NOT NULL,
    pilot          TEXT NOT NULL,
    path           TEXT NOT NULL,
    symbol_path    TEXT,
    target_branch  TEXT NOT NULL,
    origin         TEXT NOT NULL,
    acquired_at    INTEGER NOT NULL,
    released_at    INTEGER NOT NULL,
    release_reason TEXT NOT NULL CHECK (release_reason IN ('released', 'expired', 'session_end'))
) STRICT;

CREATE INDEX claim_history_by_seat ON claim_history (seat_id, acquired_at);

-- One row per hook event. Structural data only: never prompts, never file contents. `paths` is a
-- JSON array of strings.
CREATE TABLE events (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    seat_id     TEXT NOT NULL REFERENCES seats (seat_id),
    at          INTEGER NOT NULL,
    phase       TEXT NOT NULL,
    tool_name   TEXT,
    tool_kind   TEXT,
    paths       TEXT NOT NULL DEFAULT '[]',
    reason_code TEXT,
    outcome     TEXT,
    detail      TEXT
) STRICT;

CREATE INDEX events_by_seat ON events (seat_id, id);
CREATE INDEX events_by_time ON events (at);

CREATE TABLE notices (
    notice_id    TEXT PRIMARY KEY,
    kind         TEXT NOT NULL,
    target_seat  TEXT NOT NULL REFERENCES seats (seat_id),
    body         TEXT NOT NULL,
    created_at   INTEGER NOT NULL,
    delivered_at INTEGER
) STRICT;

-- The hook asks only for undelivered notices of one seat.
CREATE INDEX notices_pending ON notices (target_seat, created_at) WHERE delivered_at IS NULL;

CREATE TABLE stats (
    counter TEXT PRIMARY KEY,
    value   INTEGER NOT NULL
) STRICT;
