-- Per-project sub-issue auto-close (Linear: off by default).
ALTER TABLE projects ADD COLUMN auto_close_sub_issues INTEGER NOT NULL DEFAULT 0;
