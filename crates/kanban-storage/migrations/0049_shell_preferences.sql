-- 0049 shell preferences
--
-- How the operator keeps the surface arranged (KAN-S5-US1,
-- KAN-S5-US3): whether the navigation rail stands open, and which
-- board columns are collapsed to their rail in each scope. This is
-- per-operator data in the authoritative store rather than browser
-- state, so the arrangement survives a reload, a new window, and a
-- cleared browser origin. A Saved View owns hidden columns and
-- expanded groups; collapse is owned by nothing else, so it lives
-- here beside the rail.
--
-- One row holds the whole arrangement, so one optimistic version
-- guards every part of it and a write never leaves half an
-- arrangement behind. No row is seeded: a shell nobody has arranged
-- reads the everyday arrangement, and absence stays the honest record
-- of that.

CREATE TABLE shell_preferences (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    rail_open INTEGER NOT NULL CHECK (rail_open IN (0, 1)),
    version INTEGER NOT NULL CHECK (version > 0)
);

-- The columns one scope keeps collapsed, one row per column. The
-- scope is a kind plus a Project identity — 0 for the global scope —
-- so the key holds NULLs nowhere, and the column names are the
-- vocabulary's own, spelled exactly as the domain fixes them.
CREATE TABLE shell_collapsed_columns (
    scope_kind TEXT NOT NULL CHECK (scope_kind IN ('global', 'project')),
    project_id INTEGER NOT NULL DEFAULT 0,
    column_name TEXT NOT NULL CHECK (
        column_name IN (
            'draft', 'backlog', 'parked', 'blocked', 'scheduled', 'ready',
            'current', 'review', 'staged', 'approved', 'landing', 'done'
        )
    ),
    PRIMARY KEY (scope_kind, project_id, column_name)
);
