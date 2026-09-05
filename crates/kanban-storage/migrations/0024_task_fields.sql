-- 0024 task fields
--
-- KAN-T19: a Task is bounded work. It names one subtype of the closed
-- set, a human-or-agent mode, and the completion criteria that bound
-- it — never story-linked criteria — and stores optional schedule or
-- due-date timing for KAN-S11, whose activation behaviour lands with
-- KAN-T53 and KAN-T54 (DR-TK-06, DR-TK-07). The `tickets` rebuild
-- adds the five Task columns, carries 0023's four Bug columns and
-- restates its two shape triggers across the rebuild that would
-- otherwise drop them, and keeps each kind to exactly its own
-- fields: a Task row carries the closed subtype and mode vocabularies
-- and a non-empty completion list, and Implementation and Bug rows
-- carry none of the five. Task rows recorded before this migration
-- named none of these facts, so the copy backfills them: the
-- vocabulary-first subtype operational, the human mode of the
-- operator who created them, and one completion criterion recording
-- the backfill itself.

CREATE TABLE tickets_bounded (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id     INTEGER NOT NULL REFERENCES projects (id),
    number         INTEGER NOT NULL CHECK (number > 0),
    kind           TEXT NOT NULL CHECK (kind IN ('implementation', 'bug', 'task')),
    priority       TEXT NOT NULL CHECK (priority IN ('urgent', 'high', 'normal', 'low')),
    state          TEXT NOT NULL CHECK (state IN (
        'draft', 'parked', 'blocked', 'scheduled', 'ready', 'active',
        'in_review', 'approved', 'landing', 'done', 'cancelled',
        'superseded'
    )),
    spec_id        INTEGER REFERENCES specs (id),
    title          TEXT,
    slice          TEXT,
    criteria       TEXT NOT NULL DEFAULT '[]',
    actual_behaviour  TEXT,
    reporter_evidence TEXT,
    bug_qualification TEXT,
    bug_facts         TEXT,
    subtype        TEXT,
    mode           TEXT,
    completion     TEXT NOT NULL DEFAULT '[]',
    scheduled_for  TEXT,
    due            TEXT,
    version        INTEGER NOT NULL CHECK (version > 0),
    created_at     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
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

INSERT INTO tickets_bounded (
    id, project_id, number, kind, priority, state, spec_id, title, slice,
    criteria, actual_behaviour, reporter_evidence, bug_qualification, bug_facts,
    subtype, mode, completion, scheduled_for, due, version, created_at
)
SELECT
    id, project_id, number, kind, priority, state, spec_id, title, slice,
    criteria, actual_behaviour, reporter_evidence, bug_qualification, bug_facts,
    CASE WHEN kind = 'task' THEN 'operational' END,
    CASE WHEN kind = 'task' THEN 'human' END,
    CASE WHEN kind = 'task'
        THEN '["Completion criteria recorded by migration 0024; this Task predates bounded creation"]'
        ELSE '[]'
    END,
    NULL, NULL,
    version, created_at
FROM tickets;

DROP TABLE tickets;
ALTER TABLE tickets_bounded RENAME TO tickets;

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
