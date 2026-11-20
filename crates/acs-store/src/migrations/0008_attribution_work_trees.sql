-- The work tree each attribution's path is relative to (issue 74, decision 0050).
--
-- A session started in the main checkout can change files in a linked worktree; their paths are
-- recorded as that worktree calls them (decision 0032). `work_tree` is the absolute root of the
-- work tree the change was made in, so `acs undo` acts there whichever checkout it runs in. NULL
-- for rows written before this column existed: they belong to the work tree undo runs in.

ALTER TABLE attributions ADD COLUMN work_tree TEXT;
