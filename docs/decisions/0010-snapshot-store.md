# 0010 — Snapshot store

Date: 2026-10-29. Status: accepted (WP1.6).

## Decision

**Layout.** `snapshots/index.db` (SQLite, WAL, 5 s busy timeout, `PRAGMA user_version`
migrations; decision 0004 rules 9 and 10) and `snapshots/blobs/<2 hex>/<62 hex>`. A blob is named by
the SHA-256 of the *uncompressed* content (`acs_core::ContentHash`) and holds a zstd frame (level 1:
capture runs in the pre-tool hook). Files and directories are owner-only (`fsutil`). Blobs are
written with temp file + rename and never rewritten when present.

**Index.** `blobs(hash, size, stored, last_access)` and `entries(id, session, path, kind, taken_at,
hash NULL = absent, size, mode)`. `entries.id` is the capture sequence. The first pre-image of a
path in a session is its pre-image; the latest post-image is its post-image. Every capture is
recorded, including repeated pre-images.

**Concurrency and crashes.** Capture runs in one `BEGIN IMMEDIATE` transaction: for each file it
reads and hashes, stats the blob (dedupe: existing content is not compressed or rewritten), writes
the blob, then upserts the `blobs` row and inserts the entry. The blob always precedes its rows.
`gc` removes index rows in one transaction, then in a second write transaction deletes blob files
that have no row, so a capture that raced in between keeps its blob. Leftover blob files and
temporary files from crashes are swept by the same pass; no grace period is needed because blob
writes happen only under the write lock.

**Eviction.** `gc(budget, live_sessions)` first drops blobs no entry refers to, then evicts
least-recently-used blobs (by `last_access`, set on capture and on restore) by compressed size until
the total is within the budget. Blobs referenced by any entry of a live session are never evicted;
if they alone exceed the budget the report says `over_budget`. Entries of evicted blobs are
**kept** and report `SnapshotError::Evicted` ("snapshot evicted") on read or restore, so undo can
say so instead of failing obscurely. `MissingBlob` (indexed, file deleted behind our back) and
`Corrupt` (does not decompress, or the content does not match its hash) are separate errors; blobs
are verified on every read. `drop_session` removes a finished session's entries so its blobs become
collectable.

**Modes.** Unix: `st_mode & 0o7777`, restored with `chmod` on the temporary file before an atomic
rename over the destination (so restoring over a read-only file works). Windows: only the read-only
flag, recorded as `0o444` / `0o644`; the flag is cleared on an existing destination before the
rename. Symlinks are followed by `capture`; `capture_tree` skips them. Directories passed to
`capture` are an error; use `capture_tree`. An empty or missing directory yields no entries
(deleted empty directories are not restored).

**Dependencies.** `zstd` 0.14.3 (BSD-3-Clause, builds C zstd with the `cc` crate on Linux, macOS and
Windows) and `rusqlite` 0.41.1 `bundled` (MIT), both in `[workspace.dependencies]`.

## Measured

Capturing 1,000 small files (release build, 4 shared CPUs): cold 71 ms, all content already stored
12 ms. Debug build: 91 ms / 37 ms. The test asserts 200 ms in release and 5 s in debug.
