-- 0006 timeline scope
--
-- Every timeline row states the scope it belongs to: `global` for
-- entities that sit above every Project, `project` for everything
-- recorded inside one (KAN-S2-US1).
--
-- Initiative history was written with an empty Project identity and
-- uncatalogued `initiative.*` kinds, so the query surface could
-- never read it back. Repair those rows in place: the scope becomes
-- global, the kind becomes the closed `transition`, and the
-- transition it recorded moves into the detail as an action. Each
-- row keeps its identity, its recorded time, and every fact it
-- already carried (KAN-S2-US5).

ALTER TABLE timeline_events ADD COLUMN scope TEXT NOT NULL DEFAULT 'project';

-- The append-only guard exists to refuse application writes. This
-- repair is schema evolution, so it stands the trigger down for the
-- length of the migration and puts it straight back.
DROP TRIGGER timeline_events_append_only_update;

UPDATE timeline_events
   SET scope = 'global'
 WHERE project_id = '';

UPDATE timeline_events
   SET kind = 'transition',
       detail = json_set(detail, '$.action', substr(kind, length('initiative.') + 1))
 WHERE kind LIKE 'initiative.%';

CREATE TRIGGER timeline_events_append_only_update
BEFORE UPDATE ON timeline_events
BEGIN
    SELECT RAISE(ABORT, 'timeline_events is append-only');
END;

-- Queries always name a scope, so the scope leads the index that
-- replaces the Project-only one.
DROP INDEX idx_timeline_events_project_recorded;

CREATE INDEX idx_timeline_events_scope_recorded
    ON timeline_events (scope, project_id, recorded_at);
