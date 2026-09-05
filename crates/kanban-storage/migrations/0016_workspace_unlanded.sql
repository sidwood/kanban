-- 0016 workspace unlanded guard
--
-- Observation records whether a Workspace holds unique unlanded
-- commits so reuse evaluation can refuse it (DR-LW-06). NULL means
-- the observer could not decide; nothing here deletes a Workspace.

ALTER TABLE workspaces ADD COLUMN unique_unlanded_commits INTEGER
    CHECK (unique_unlanded_commits IN (0, 1));
