-- 0053 linear issue model
--
-- Applied idempotently from `src/linear_issue_migration.rs` so recovery
-- tests can re-run migrations 47+ without failing on existing tables.
-- This file documents the intended DDL; keep it aligned with that module.

SELECT 1;
