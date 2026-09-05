-- 0011 plans
--
-- A Plan is a versioned, ordered dependency graph of Specs: the
-- working shape lives in plan_specs (position = display order) and
-- plan_edges (a separate relation), and every activation freezes one
-- immutable snapshot into plan_versions with its own spec and edge
-- rows. Plans are archived, never deleted; the trigger refuses plan
-- deletes at the schema level and the frozen version rows are
-- append-only, so prior versions stay queryable beside every
-- replacement.

CREATE TABLE plans (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id INTEGER NOT NULL REFERENCES projects (id),
    number     INTEGER NOT NULL CHECK (number > 0),
    state      TEXT NOT NULL
        CHECK (state IN ('draft', 'active', 'complete', 'cancelled', 'archived')),
    version    INTEGER NOT NULL CHECK (version > 0),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (project_id, number)
);

CREATE TRIGGER plans_refuse_delete
BEFORE DELETE ON plans
BEGIN
    SELECT RAISE(ABORT, 'plans are archived, never deleted');
END;

CREATE TABLE plan_specs (
    plan_id     INTEGER NOT NULL REFERENCES plans (id),
    position    INTEGER NOT NULL CHECK (position >= 0),
    spec_number INTEGER NOT NULL CHECK (spec_number > 0),
    PRIMARY KEY (plan_id, spec_number)
);

CREATE INDEX plan_specs_display_order ON plan_specs (plan_id, position);

CREATE TABLE plan_edges (
    plan_id   INTEGER NOT NULL REFERENCES plans (id),
    from_spec INTEGER NOT NULL CHECK (from_spec > 0),
    to_spec   INTEGER NOT NULL CHECK (to_spec > 0),
    CHECK (from_spec <> to_spec),
    PRIMARY KEY (plan_id, from_spec, to_spec)
);

CREATE TABLE plan_versions (
    id      INTEGER PRIMARY KEY AUTOINCREMENT,
    plan_id INTEGER NOT NULL REFERENCES plans (id),
    number  INTEGER NOT NULL CHECK (number > 0),
    UNIQUE (plan_id, number)
);

CREATE TABLE plan_version_specs (
    version_id  INTEGER NOT NULL REFERENCES plan_versions (id),
    position    INTEGER NOT NULL CHECK (position >= 0),
    spec_number INTEGER NOT NULL CHECK (spec_number > 0),
    PRIMARY KEY (version_id, spec_number)
);

CREATE INDEX plan_version_specs_display_order
    ON plan_version_specs (version_id, position);

CREATE TABLE plan_version_edges (
    version_id INTEGER NOT NULL REFERENCES plan_versions (id),
    from_spec  INTEGER NOT NULL CHECK (from_spec > 0),
    to_spec    INTEGER NOT NULL CHECK (to_spec > 0),
    CHECK (from_spec <> to_spec),
    PRIMARY KEY (version_id, from_spec, to_spec)
);
