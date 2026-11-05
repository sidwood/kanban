-- 0066 label and milestone name case
--
-- Label and milestone names are unique within their project ignoring
-- letter case. The application judges that with Unicode case folding
-- before it writes; these triggers back the rule in the database for
-- the ASCII letters SQLite's NOCASE collation folds, so a write that
-- skips the application is still refused.
--
-- The triggers judge only later writes. A database written before this
-- rule may already hold names that differ only in letter case, and
-- every such row is kept exactly as it is: still listed, still held by
-- its issues, and still assignable by its exact spelling. Nothing is
-- renamed, merged, or dropped, so applying this migration cannot fail
-- on existing data.
--
-- An exact repeat is left to the existing UNIQUE (project_id, name)
-- constraint, so INSERT OR IGNORE seeding keeps ignoring it. A rename
-- that changes only a row's own letter case stays free, and a row
-- whose name and project stand still stays writable even when it
-- belongs to an older case-variant pair.

CREATE TRIGGER IF NOT EXISTS issue_labels_name_case_insert
BEFORE INSERT ON issue_labels
WHEN NOT EXISTS (
    SELECT 1 FROM issue_labels
    WHERE project_id = NEW.project_id AND name = NEW.name
) AND EXISTS (
    SELECT 1 FROM issue_labels
    WHERE project_id = NEW.project_id AND name = NEW.name COLLATE NOCASE
)
BEGIN
    SELECT RAISE(ABORT, 'label name differs only in letter case from a stored label');
END;

CREATE TRIGGER IF NOT EXISTS issue_labels_name_case_update
BEFORE UPDATE OF project_id, name ON issue_labels
WHEN (NEW.name IS NOT OLD.name OR NEW.project_id IS NOT OLD.project_id)
AND NOT EXISTS (
    SELECT 1 FROM issue_labels
    WHERE project_id = NEW.project_id AND name = NEW.name AND id <> NEW.id
) AND EXISTS (
    SELECT 1 FROM issue_labels
    WHERE project_id = NEW.project_id
      AND name = NEW.name COLLATE NOCASE
      AND id <> NEW.id
)
BEGIN
    SELECT RAISE(ABORT, 'label name differs only in letter case from a stored label');
END;

CREATE TRIGGER IF NOT EXISTS issue_milestones_name_case_insert
BEFORE INSERT ON issue_milestones
WHEN NOT EXISTS (
    SELECT 1 FROM issue_milestones
    WHERE project_id = NEW.project_id AND name = NEW.name
) AND EXISTS (
    SELECT 1 FROM issue_milestones
    WHERE project_id = NEW.project_id AND name = NEW.name COLLATE NOCASE
)
BEGIN
    SELECT RAISE(ABORT, 'milestone name differs only in letter case from a stored milestone');
END;

CREATE TRIGGER IF NOT EXISTS issue_milestones_name_case_update
BEFORE UPDATE OF project_id, name ON issue_milestones
WHEN (NEW.name IS NOT OLD.name OR NEW.project_id IS NOT OLD.project_id)
AND NOT EXISTS (
    SELECT 1 FROM issue_milestones
    WHERE project_id = NEW.project_id AND name = NEW.name AND id <> NEW.id
) AND EXISTS (
    SELECT 1 FROM issue_milestones
    WHERE project_id = NEW.project_id
      AND name = NEW.name COLLATE NOCASE
      AND id <> NEW.id
)
BEGIN
    SELECT RAISE(ABORT, 'milestone name differs only in letter case from a stored milestone');
END;
