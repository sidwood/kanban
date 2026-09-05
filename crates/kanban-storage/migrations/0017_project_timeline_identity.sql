-- 0017 project timeline identity
--
-- Every project-scoped row now names its Project by the numeric
-- identity the application resolved through the Project store
-- (KAN-T79). The earlier commands wrote whatever string the request
-- carried, so one Project's permanent history could sit split across
-- several unrelated scopes.
--
-- Repair those rows in place. When exactly one Project exists, a
-- scope that names no Project row can only be that Project, so the
-- timeline rows and the entity rows rejoin its identity; each keeps
-- its own identity, its recorded time, and every fact it carried.
-- When several Projects exist, no rule can attribute a split scope
-- without guessing, and a wrong guess would misplace permanent
-- history, so those rows stay exactly where they were and the audit
-- trail names every scope left behind. Nothing is dropped either
-- way.

DROP TRIGGER timeline_events_append_only_update;
DROP TRIGGER rulings_append_only_update;
DROP TRIGGER deferrals_append_only_update;
DROP TRIGGER evidence_items_append_only_update;

UPDATE timeline_events
   SET project_id = (SELECT CAST(id AS TEXT) FROM projects ORDER BY id LIMIT 1)
 WHERE (SELECT COUNT(*) FROM projects) = 1
   AND scope = 'project'
   AND project_id NOT IN (SELECT CAST(id AS TEXT) FROM projects);

UPDATE comments
   SET project_id = (SELECT CAST(id AS TEXT) FROM projects ORDER BY id LIMIT 1)
 WHERE (SELECT COUNT(*) FROM projects) = 1
   AND project_id NOT IN (SELECT CAST(id AS TEXT) FROM projects);

UPDATE rulings
   SET project_id = (SELECT CAST(id AS TEXT) FROM projects ORDER BY id LIMIT 1)
 WHERE (SELECT COUNT(*) FROM projects) = 1
   AND project_id NOT IN (SELECT CAST(id AS TEXT) FROM projects);

UPDATE deferrals
   SET project_id = (SELECT CAST(id AS TEXT) FROM projects ORDER BY id LIMIT 1)
 WHERE (SELECT COUNT(*) FROM projects) = 1
   AND project_id NOT IN (SELECT CAST(id AS TEXT) FROM projects);

UPDATE evidence_items
   SET project_id = (SELECT CAST(id AS TEXT) FROM projects ORDER BY id LIMIT 1)
 WHERE (SELECT COUNT(*) FROM projects) = 1
   AND project_id NOT IN (SELECT CAST(id AS TEXT) FROM projects);

-- Whatever could not be attributed stays put, and the audit trail
-- says so: the scopes are preserved verbatim, never silently
-- dropped. An empty candidate set inserts no row.
INSERT INTO audit_events (kind, detail)
SELECT 'timeline.scopes.unattributed',
       json_object('scopes', json_group_array(DISTINCT scope))
  FROM (
      SELECT project_id AS scope FROM timeline_events WHERE scope = 'project'
      UNION
      SELECT project_id FROM comments
      UNION
      SELECT project_id FROM rulings
      UNION
      SELECT project_id FROM deferrals
      UNION
      SELECT project_id FROM evidence_items
  )
 WHERE scope NOT IN (SELECT CAST(id AS TEXT) FROM projects)
HAVING COUNT(*) > 0;

CREATE TRIGGER timeline_events_append_only_update
BEFORE UPDATE ON timeline_events
BEGIN
    SELECT RAISE(ABORT, 'timeline_events is append-only');
END;

CREATE TRIGGER rulings_append_only_update
BEFORE UPDATE ON rulings
BEGIN
    SELECT RAISE(ABORT, 'rulings is append-only');
END;

CREATE TRIGGER deferrals_append_only_update
BEFORE UPDATE ON deferrals
BEGIN
    SELECT RAISE(ABORT, 'deferrals is append-only');
END;

CREATE TRIGGER evidence_items_append_only_update
BEFORE UPDATE ON evidence_items
BEGIN
    SELECT RAISE(ABORT, 'evidence_items is append-only');
END;
