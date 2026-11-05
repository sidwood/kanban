-- 0062 canonical comment addresses
--
-- Legacy comment rows on tickets carry whichever spelling their
-- caller passed; the listing now filters by the canonical numeric id
-- in SQL. Applied from `src/linear_issue_migration.rs`, which
-- rewrites every resolvable spelling to the numeric id of the issue
-- it addresses. This file documents the intent; keep it aligned.

SELECT 1;
