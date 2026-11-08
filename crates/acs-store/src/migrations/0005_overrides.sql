-- Human overrides of `owned` territory (WP7.1, decision 0026) and every use of them.
--
-- An override is granted by a human with `acs override`. Until a seat uses it, it is unbound and
-- lapses at `expires_at`. A `once` override is used up by the first tool call that needs it
-- (`consumed_at`); a `session` override is bound to the first seat that uses it (`seat_id`) and
-- lasts as long as that seat. Rows are never deleted: they are the record of who let whom in.

CREATE TABLE overrides (
    override_id TEXT PRIMARY KEY,
    pattern     TEXT NOT NULL,
    mode        TEXT NOT NULL CHECK (mode IN ('once', 'session')),
    granted_by  TEXT NOT NULL,
    granted_at  INTEGER NOT NULL,
    expires_at  INTEGER NOT NULL,
    seat_id     TEXT,
    consumed_at INTEGER,
    revoked_at  INTEGER
) STRICT;

CREATE INDEX overrides_open ON overrides (consumed_at, revoked_at);

-- One row per path an override let through.
CREATE TABLE override_uses (
    override_id TEXT NOT NULL REFERENCES overrides (override_id),
    seat_id     TEXT NOT NULL,
    path        TEXT NOT NULL,
    at          INTEGER NOT NULL
) STRICT;

CREATE INDEX override_uses_by_seat ON override_uses (seat_id, at);
