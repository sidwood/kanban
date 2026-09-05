-- 0019 workspace detached head
--
-- Detached checkouts are an explicit closed state (KAN-T98, KAN-S6).
-- Rows recorded before this migration stored the artifact that
-- `git rev-parse --abbrev-ref HEAD` prints for a detached checkout —
-- the literal string `HEAD` — as a branch name. The flag marks the
-- detached state, those rows are rewritten to it, and `branch` holds
-- a branch name again or nothing at all.

ALTER TABLE workspaces ADD COLUMN detached INTEGER NOT NULL DEFAULT 0
    CHECK (detached IN (0, 1));

UPDATE workspaces SET detached = 1, branch = NULL WHERE branch = 'HEAD';
