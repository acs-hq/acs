-- The remote-claims view: other machines' claims as last fetched, and when the fetch worked.

CREATE TABLE remote_claims (
    pilot_id      TEXT NOT NULL,
    claim_id      TEXT NOT NULL,
    seat_id       TEXT NOT NULL,
    pilot         TEXT NOT NULL,
    path          TEXT NOT NULL,
    symbol_path   TEXT,
    target_branch TEXT NOT NULL,
    acquired_at   INTEGER NOT NULL,
    heartbeat_at  INTEGER NOT NULL,
    ttl_secs      INTEGER NOT NULL,
    expires_at    INTEGER NOT NULL,
    origin        TEXT NOT NULL CHECK (origin IN ('agent', 'human')),
    fetched_at    INTEGER NOT NULL,
    PRIMARY KEY (pilot_id, claim_id)
) STRICT, WITHOUT ROWID;

CREATE INDEX remote_claims_by_path ON remote_claims (path, target_branch, expires_at);

-- At most one row (id = 1): the sync status.
CREATE TABLE remote_sync (
    id              INTEGER PRIMARY KEY CHECK (id = 1),
    last_success_at INTEGER,
    last_attempt_at INTEGER,
    last_error      TEXT
) STRICT;
