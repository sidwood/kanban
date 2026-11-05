-- 0063 block edges share the dependency graph
--
-- Operator block links written only to issue_relations move into
-- ticket_dependencies, the table dispatch readiness reads. Related
-- links stay in issue_relations. Repeating the statement changes
-- nothing.

INSERT OR IGNORE INTO ticket_dependencies (from_ticket, to_ticket)
SELECT from_ticket_id, to_ticket_id
FROM issue_relations
WHERE kind = 'blocks';

DELETE FROM issue_relations WHERE kind = 'blocks';
