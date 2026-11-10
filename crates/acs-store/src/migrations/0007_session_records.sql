-- Which seats already have a session record in the ledger (WP4.1).
--
-- A seat is recorded when it ends, or when it looks crashed. `recorded_last_seen_at` is the seat's
-- `last_seen_at` at the time: a seat that comes back (resumed, or first recorded as crashed) moves
-- past it and gets a new record with the same session id. A seat with nothing worth recording has
-- a row without `ledger_commit`, so it is not looked at again.

CREATE TABLE session_records (
    seat_id               TEXT PRIMARY KEY REFERENCES seats (seat_id),
    session_id            TEXT NOT NULL,
    recorded_last_seen_at INTEGER NOT NULL,
    ended_reason          TEXT NOT NULL CHECK (ended_reason IN ('clean', 'crashed')),
    ledger_commit         TEXT,
    signed                INTEGER NOT NULL DEFAULT 0 CHECK (signed IN (0, 1)),
    written_at            INTEGER NOT NULL
) STRICT;
