-- 0014 specs
--
-- A Spec is the lightweight PRD for one behaviour area: rows in
-- `specs` carry the Project, the minted number, the execution state
-- tracked separately from content, and the Plan binding set at
-- planning. Content versions live in `spec_versions`, one row per
-- version carrying all nine PRD sections plus the version state.
-- Draft rows are editable; approved and superseded rows are
-- immutable except for the explicit forward-only state moves
-- (draft → approved → superseded), so a Ticket pinned to any version
-- keeps resolving. Specs are never deleted.

CREATE TABLE specs (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id INTEGER NOT NULL REFERENCES projects (id),
    number     INTEGER NOT NULL CHECK (number > 0),
    execution  TEXT NOT NULL CHECK (
        execution IN (
            'unplanned', 'planned', 'blocked', 'ready', 'active',
            'integration_review', 'complete', 'cancelled'
        )
    ),
    plan_id INTEGER REFERENCES plans (id),
    version INTEGER NOT NULL CHECK (version > 0),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (project_id, number)
);

CREATE TRIGGER specs_refuse_delete
BEFORE DELETE ON specs
BEGIN
    SELECT RAISE(ABORT, 'specs are never deleted; supersede or cancel them');
END;

CREATE TABLE spec_versions (
    id       INTEGER PRIMARY KEY AUTOINCREMENT,
    spec_id  INTEGER NOT NULL REFERENCES specs (id),
    number   INTEGER NOT NULL CHECK (number > 0),
    state    TEXT NOT NULL CHECK (state IN ('draft', 'approved', 'superseded')),
    name     TEXT NOT NULL,
    short_description      TEXT NOT NULL,
    problem_statement      TEXT NOT NULL,
    solution               TEXT NOT NULL,
    user_stories           TEXT NOT NULL,
    implementation_decisions TEXT NOT NULL,
    testing_decisions      TEXT NOT NULL,
    out_of_scope           TEXT NOT NULL,
    further_notes          TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (spec_id, number)
);

CREATE TRIGGER spec_versions_refuse_delete
BEFORE DELETE ON spec_versions
BEGIN
    SELECT RAISE(ABORT, 'spec versions are never deleted');
END;

-- Approved and superseded content is immutable (DR-PS-09): once a
-- version leaves draft, rewriting any PRD section is refused at the
-- schema level. Only the state column may still move, forward.
CREATE TRIGGER spec_versions_frozen_content
BEFORE UPDATE ON spec_versions
WHEN OLD.state IN ('approved', 'superseded')
     AND (NEW.name IS NOT OLD.name
          OR NEW.short_description IS NOT OLD.short_description
          OR NEW.problem_statement IS NOT OLD.problem_statement
          OR NEW.solution IS NOT OLD.solution
          OR NEW.user_stories IS NOT OLD.user_stories
          OR NEW.implementation_decisions IS NOT OLD.implementation_decisions
          OR NEW.testing_decisions IS NOT OLD.testing_decisions
          OR NEW.out_of_scope IS NOT OLD.out_of_scope
          OR NEW.further_notes IS NOT OLD.further_notes)
BEGIN
    SELECT RAISE(ABORT, 'approved and superseded Spec versions are immutable');
END;

-- Version states move one way: draft → approved → superseded, with
-- draft → superseded as the explicit abandon. Nothing moves back,
-- and superseded is terminal.
CREATE TRIGGER spec_versions_state_moves_forward
BEFORE UPDATE ON spec_versions
WHEN NOT (
    (OLD.state = 'draft' AND NEW.state IN ('draft', 'approved', 'superseded'))
    OR (OLD.state = 'approved' AND NEW.state IN ('approved', 'superseded'))
    OR (OLD.state = 'superseded' AND NEW.state = 'superseded')
)
BEGIN
    SELECT RAISE(ABORT, 'spec version states move draft, approved, superseded');
END;
