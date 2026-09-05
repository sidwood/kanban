-- Herdr observation settings and global defaults (KAN-S8).

CREATE TABLE herdr_global_defaults (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    reconciliation_interval_secs INTEGER NOT NULL DEFAULT 300,
    stall_deadline_secs INTEGER NOT NULL DEFAULT 3600,
    missing_result_deadline_secs INTEGER NOT NULL DEFAULT 7200,
    version INTEGER NOT NULL DEFAULT 1
);

INSERT INTO herdr_global_defaults (
    id,
    reconciliation_interval_secs,
    stall_deadline_secs,
    missing_result_deadline_secs,
    version
) VALUES (1, 300, 3600, 7200, 1);

CREATE TABLE herdr_project_settings (
    project_id INTEGER PRIMARY KEY REFERENCES projects(id),
    reconciliation_interval_secs INTEGER NOT NULL,
    polling_fallback_enabled INTEGER NOT NULL DEFAULT 0,
    polling_fallback_interval_secs INTEGER NOT NULL DEFAULT 10,
    stall_deadline_secs INTEGER NOT NULL,
    missing_result_deadline_secs INTEGER NOT NULL,
    version INTEGER NOT NULL DEFAULT 1
);

-- Every Project that already existed before this migration inherits
-- the global defaults; fresh registrations still seed at insert time.
INSERT INTO herdr_project_settings (
    project_id,
    reconciliation_interval_secs,
    polling_fallback_enabled,
    polling_fallback_interval_secs,
    stall_deadline_secs,
    missing_result_deadline_secs,
    version
)
SELECT
    projects.id,
    herdr_global_defaults.reconciliation_interval_secs,
    0,
    10,
    herdr_global_defaults.stall_deadline_secs,
    herdr_global_defaults.missing_result_deadline_secs,
    1
FROM projects
CROSS JOIN herdr_global_defaults
WHERE herdr_global_defaults.id = 1;
