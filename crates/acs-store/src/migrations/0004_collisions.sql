-- `acs status` lists recent collisions. The partial index keeps that independent of the size of the
-- event log, which grows with every tool call.
CREATE INDEX events_collisions ON events (at) WHERE reason_code = 'claim_conflict';
