-- 0060 linear issue fidelity
--
-- Applied idempotently from `src/linear_issue_migration.rs` so databases
-- that already applied an earlier build of migrations 53 through 59 are
-- repaired: legacy due timestamps become calendar dates, terminal stamps
-- are backfilled from the state history, every issue keeps a history row
-- for its current state, and the Severity and Mode label groups become
-- single-select like the Type group.

SELECT 1;
