-- Lore shown to each seat (WP6.1). A seat is one harness session, so a row means "this session
-- has had this entry"; the primary key makes the once-per-session rule hold even when two hooks
-- of one seat race. `stale` records whether the entry was shown with a request to verify it.
-- Session records take `lore_loaded` from here (WP4.1).

CREATE TABLE lore_loaded (
    seat_id   TEXT NOT NULL,
    lore_id   TEXT NOT NULL,
    loaded_at INTEGER NOT NULL,
    stale     INTEGER NOT NULL DEFAULT 0 CHECK (stale IN (0, 1)),
    PRIMARY KEY (seat_id, lore_id)
) STRICT;
