-- 0012 plan version append-only refusals
--
-- The frozen Plan version tables are the audit of every approved
-- shape: 0011 named them append-only but enforced nothing beyond the
-- UNIQUE keys. These triggers refuse UPDATE and DELETE on
-- plan_versions, plan_version_specs, and plan_version_edges at the
-- schema level, so a frozen version can only ever be appended to,
-- never rewritten or dropped.

CREATE TRIGGER plan_versions_append_only_update
BEFORE UPDATE ON plan_versions
BEGIN
    SELECT RAISE(ABORT, 'plan_versions is append-only');
END;

CREATE TRIGGER plan_versions_append_only_delete
BEFORE DELETE ON plan_versions
BEGIN
    SELECT RAISE(ABORT, 'plan_versions is append-only');
END;

CREATE TRIGGER plan_version_specs_append_only_update
BEFORE UPDATE ON plan_version_specs
BEGIN
    SELECT RAISE(ABORT, 'plan_version_specs is append-only');
END;

CREATE TRIGGER plan_version_specs_append_only_delete
BEFORE DELETE ON plan_version_specs
BEGIN
    SELECT RAISE(ABORT, 'plan_version_specs is append-only');
END;

CREATE TRIGGER plan_version_edges_append_only_update
BEFORE UPDATE ON plan_version_edges
BEGIN
    SELECT RAISE(ABORT, 'plan_version_edges is append-only');
END;

CREATE TRIGGER plan_version_edges_append_only_delete
BEFORE DELETE ON plan_version_edges
BEGIN
    SELECT RAISE(ABORT, 'plan_version_edges is append-only');
END;
