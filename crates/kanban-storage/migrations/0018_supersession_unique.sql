-- 0011 supersession uniqueness
--
-- One non-null successor per original for rulings and deferrals
-- (KAN-S2-US3, DR-AE-03). Supersession forms a single unambiguous
-- chain; legacy stores may already hold duplicate successors and
-- are recovered deterministically before the unique indexes apply.

-- Preserves duplicate successor rows the recovery detaches.
CREATE TABLE supersession_duplicate_quarantine (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    record_kind   TEXT NOT NULL CHECK (record_kind IN ('ruling', 'deferral')),
    successor_id  INTEGER NOT NULL,
    supersedes_id INTEGER NOT NULL,
    project_id    TEXT NOT NULL,
    detail        TEXT NOT NULL,
    recovered_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

-- Rulings: quarantine every non-canonical duplicate successor.
INSERT INTO supersession_duplicate_quarantine (
    record_kind, successor_id, supersedes_id, project_id, detail
)
SELECT
    'ruling',
    id,
    supersedes_id,
    project_id,
    json_object(
        'entity_kind', entity_kind,
        'entity_id', entity_id,
        'summary', summary,
        'recorded_at', recorded_at
    )
FROM (
    SELECT
        id,
        supersedes_id,
        project_id,
        entity_kind,
        entity_id,
        summary,
        recorded_at,
        ROW_NUMBER() OVER (
            PARTITION BY supersedes_id
            ORDER BY recorded_at ASC, id ASC
        ) AS successor_rank
    FROM rulings
    WHERE supersedes_id IS NOT NULL
) ranked
WHERE successor_rank > 1;

-- Deferrals: quarantine every non-canonical duplicate successor.
INSERT INTO supersession_duplicate_quarantine (
    record_kind, successor_id, supersedes_id, project_id, detail
)
SELECT
    'deferral',
    id,
    supersedes_id,
    project_id,
    json_object(
        'finding_id', finding_id,
        'reason', reason,
        'recorded_at', recorded_at
    )
FROM (
    SELECT
        id,
        supersedes_id,
        project_id,
        finding_id,
        reason,
        recorded_at,
        ROW_NUMBER() OVER (
            PARTITION BY supersedes_id
            ORDER BY recorded_at ASC, id ASC
        ) AS successor_rank
    FROM deferrals
    WHERE supersedes_id IS NOT NULL
) ranked
WHERE successor_rank > 1;

-- Name every quarantined identifier in the audit trail.
INSERT INTO audit_events (kind, detail)
SELECT
    'migration.supersession_recovery',
    json_object(
        'record_kind', record_kind,
        'successor_id', successor_id,
        'supersedes_id', supersedes_id,
        'project_id', project_id
    )
FROM supersession_duplicate_quarantine;

-- Schema evolution stands append-only guards down while detaching
-- duplicate successors so the unique indexes can apply.
DROP TRIGGER rulings_append_only_update;
DROP TRIGGER deferrals_append_only_update;

UPDATE rulings
   SET supersedes_id = NULL
 WHERE id IN (
     SELECT successor_id
       FROM supersession_duplicate_quarantine
      WHERE record_kind = 'ruling'
 );

UPDATE deferrals
   SET supersedes_id = NULL
 WHERE id IN (
     SELECT successor_id
       FROM supersession_duplicate_quarantine
      WHERE record_kind = 'deferral'
 );

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

CREATE UNIQUE INDEX idx_rulings_supersedes_unique
    ON rulings (supersedes_id)
    WHERE supersedes_id IS NOT NULL;

CREATE UNIQUE INDEX idx_deferrals_supersedes_unique
    ON deferrals (supersedes_id)
    WHERE supersedes_id IS NOT NULL;
