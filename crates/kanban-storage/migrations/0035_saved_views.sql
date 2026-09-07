-- 0035 saved views
--
-- Saved Views (CONTEXT.md, DR-BP-05, DR-BP-06): the named operator
-- perspectives that own the board's presentation decisions, stored
-- as per-operator data in the authoritative store rather than browser
-- state. The scope is a kind plus a Project identity — 0 for the
-- global scope — so the unique keys hold NULLs nowhere. One name per
-- scope and one default per scope are both constraints here; the
-- filter axes and the group sets persist as JSON arrays of the
-- vocabularies' wire names, spelled exactly as the domain fixes them.

CREATE TABLE saved_views (
    id INTEGER PRIMARY KEY,
    scope_kind TEXT NOT NULL CHECK (scope_kind IN ('global', 'project')),
    project_id INTEGER NOT NULL DEFAULT 0,
    name TEXT NOT NULL,
    is_default INTEGER NOT NULL DEFAULT 0,
    filter TEXT NOT NULL DEFAULT '{}',
    expanded_groups TEXT NOT NULL DEFAULT '[]',
    hidden_columns TEXT NOT NULL DEFAULT '[]',
    mode TEXT NOT NULL CHECK (mode IN ('board', 'register')),
    done TEXT NOT NULL CHECK (done IN ('column', 'table')),
    sorting TEXT NOT NULL CHECK (sorting IN ('priority', 'readiness')),
    version INTEGER NOT NULL DEFAULT 1,
    UNIQUE (scope_kind, project_id, name)
);

-- Exactly one default view per scope, whatever else the operator
-- names (DR-BP-06); the application generates the missing ones before
-- any read answers.
CREATE UNIQUE INDEX saved_views_one_default_per_scope
    ON saved_views (scope_kind, project_id)
    WHERE is_default = 1;
