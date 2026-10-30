-- The hook pipeline (WP2.1): per-seat session id and intent, the last decision per seat, repository
-- flags, and post-tool attributions.

-- `session_id` is the `ses_` id of the seat's session record (the `ACS-Session` trailer value).
-- It is nullable only because SQLite cannot add a NOT NULL column without a default; every row
-- is backfilled below and new rows always set it.
ALTER TABLE seats ADD COLUMN session_id TEXT;
ALTER TABLE seats ADD COLUMN intent TEXT;
UPDATE seats SET session_id = 'ses_' || lower(hex(randomblob(16))) WHERE session_id IS NULL;
CREATE UNIQUE INDEX seats_by_session ON seats (session_id);
-- Crashed-session detection: live seats ordered by last activity.
CREATE INDEX seats_live ON seats (last_seen_at) WHERE ended_at IS NULL;

-- The most recent pre-tool decision of each seat (MCP `acs_why`).
CREATE TABLE last_decisions (
    seat_id     TEXT PRIMARY KEY REFERENCES seats (seat_id),
    outcome     TEXT NOT NULL,
    reason_code TEXT NOT NULL,
    message     TEXT NOT NULL,
    at          INTEGER NOT NULL
) STRICT;

-- Repository-wide key/value flags, for example `gate_off`.
CREATE TABLE repo_flags (
    key        TEXT PRIMARY KEY,
    value      TEXT NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT;

-- What each tool call changed, attributed to a seat: one row per path per post-tool event.
-- `pre_hash` and `post_hash` are SHA-256 content hashes; NULL means the file did not exist (or no
-- longer exists, for `post_hash`). `via` says whether the path came from the event itself
-- (`tool`) or from the working-tree diff after a shell command (`shell`).
CREATE TABLE attributions (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    seat_id   TEXT NOT NULL REFERENCES seats (seat_id),
    path      TEXT NOT NULL,
    pre_hash  TEXT,
    post_hash TEXT,
    at        INTEGER NOT NULL,
    via       TEXT NOT NULL CHECK (via IN ('tool', 'shell'))
) STRICT;

CREATE INDEX attributions_by_seat ON attributions (seat_id, id);
CREATE INDEX attributions_by_path ON attributions (path, id);
