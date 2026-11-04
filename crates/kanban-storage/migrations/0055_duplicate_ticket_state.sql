-- 0055 duplicate ticket state
--
-- Applied from `linear_issue_migration.rs`: allow `duplicate` in `tickets.state`
-- so Duplicate workflow merges can land through `apply_free_state`.

DROP TRIGGER IF EXISTS tickets_bug_shape_insert;
DROP TRIGGER IF EXISTS tickets_bug_shape_update;
CREATE TABLE tickets_relaxed (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id INTEGER NOT NULL REFERENCES projects (id),
    number INTEGER NOT NULL CHECK (number > 0),
    kind TEXT NOT NULL CHECK (kind IN ('implementation', 'bug', 'task')),
    priority TEXT NOT NULL CHECK (priority IN ('urgent', 'high', 'normal', 'low')),
    state TEXT NOT NULL CHECK (state IN (
        'draft', 'parked', 'blocked', 'scheduled', 'ready', 'active',
        'in_review', 'approved', 'landing', 'done', 'cancelled',
        'superseded', 'duplicate'
    )),
    spec_id INTEGER REFERENCES specs (id),
    title TEXT,
    slice TEXT,
    criteria TEXT NOT NULL DEFAULT '[]',
    actual_behaviour TEXT,
    reporter_evidence TEXT,
    bug_qualification TEXT,
    bug_facts TEXT,
    subtype TEXT,
    mode TEXT,
    completion TEXT NOT NULL DEFAULT '[]',
    scheduled_for TEXT,
    due TEXT,
    profile TEXT REFERENCES execution_profiles (name),
    pinned_version INTEGER,
    predecessor_id INTEGER REFERENCES tickets (id),
    workflow_state_id INTEGER REFERENCES issue_workflow_states (id),
    linear_priority INTEGER NOT NULL DEFAULT 3
        CHECK (linear_priority BETWEEN 0 AND 4),
    due_date TEXT NOT NULL DEFAULT '2099-12-31',
    estimate REAL,
    milestone_id INTEGER REFERENCES issue_milestones (id),
    started_at TEXT,
    completed_at TEXT,
    canceled_at TEXT,
    duplicate_of_ticket_id INTEGER REFERENCES tickets (id),
    parent_ticket_id INTEGER REFERENCES tickets (id),
    sub_issue_sort REAL NOT NULL DEFAULT 0,
    description TEXT NOT NULL DEFAULT '',
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    version INTEGER NOT NULL CHECK (version > 0),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (project_id, number),
    CHECK (
        (kind = 'implementation'
            AND spec_id IS NOT NULL
            AND slice IS NOT NULL
            AND title IS NULL
            AND criteria <> '[]'
            AND subtype IS NULL
            AND mode IS NULL
            AND completion = '[]'
            AND scheduled_for IS NULL
            AND due IS NULL)
        OR (kind = 'bug'
            AND title IS NOT NULL
            AND slice IS NULL
            AND criteria = '[]'
            AND subtype IS NULL
            AND mode IS NULL
            AND completion = '[]'
            AND scheduled_for IS NULL
            AND due IS NULL)
        OR (kind = 'task'
            AND title IS NOT NULL
            AND slice IS NULL
            AND criteria = '[]'
            AND subtype IN ('operational', 'investigative', 'administrative',
                            'research', 'prototype', 'migration', 'manual')
            AND mode IN ('human', 'agent')
            AND completion <> '[]')
    )
);
INSERT INTO tickets_relaxed (
    id, project_id, number, kind, priority, state, spec_id, title, slice,
    criteria, actual_behaviour, reporter_evidence, bug_qualification, bug_facts,
    subtype, mode, completion, scheduled_for, due, profile, pinned_version,
    predecessor_id, workflow_state_id, linear_priority, due_date, estimate,
    milestone_id, started_at, completed_at, canceled_at, duplicate_of_ticket_id,
    parent_ticket_id, sub_issue_sort, description, updated_at, version, created_at
)
SELECT
    id, project_id, number, kind, priority, state, spec_id, title, slice,
    criteria, actual_behaviour, reporter_evidence, bug_qualification, bug_facts,
    subtype, mode, completion, scheduled_for, due, profile, pinned_version,
    predecessor_id, workflow_state_id, linear_priority, due_date, estimate,
    milestone_id, started_at, completed_at, canceled_at, duplicate_of_ticket_id,
    parent_ticket_id, sub_issue_sort, description, updated_at, version, created_at
FROM tickets;
DROP TABLE tickets;
ALTER TABLE tickets_relaxed RENAME TO tickets;
CREATE TRIGGER tickets_refuse_delete
BEFORE DELETE ON tickets
BEGIN
    SELECT RAISE(ABORT, 'tickets are never deleted; supersede or cancel them');
END;
CREATE TRIGGER tickets_bug_shape_insert
BEFORE INSERT ON tickets
WHEN (NEW.kind = 'bug'
        AND (NEW.actual_behaviour IS NULL
            OR NEW.reporter_evidence IS NULL
            OR NEW.bug_facts IS NULL))
    OR (NEW.kind <> 'bug'
        AND (NEW.actual_behaviour IS NOT NULL
            OR NEW.reporter_evidence IS NOT NULL
            OR NEW.bug_qualification IS NOT NULL
            OR NEW.bug_facts IS NOT NULL))
BEGIN
    SELECT RAISE(ABORT, 'each Ticket kind carries exactly its own fields');
END;
CREATE TRIGGER tickets_bug_shape_update
BEFORE UPDATE ON tickets
WHEN (NEW.kind = 'bug'
        AND (NEW.actual_behaviour IS NULL
            OR NEW.reporter_evidence IS NULL
            OR NEW.bug_facts IS NULL))
    OR (NEW.kind <> 'bug'
        AND (NEW.actual_behaviour IS NOT NULL
            OR NEW.reporter_evidence IS NOT NULL
            OR NEW.bug_qualification IS NOT NULL
            OR NEW.bug_facts IS NOT NULL))
BEGIN
    SELECT RAISE(ABORT, 'each Ticket kind carries exactly its own fields');
END;
