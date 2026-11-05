-- 0061 loose issue dates
--
-- Applied from `src/linear_issue_migration.rs` so databases that
-- already ran migration 0060 get it too: every stored due date and
-- milestone target date that parses as a calendar date is rewritten
-- zero-padded, the due-date column's in-band `2099-12-31` no-date
-- marker becomes the empty none value, and anything unparseable
-- keeps its bytes and is reported nowhere else.

SELECT 1;
