CREATE TABLE review_executions (
    id INTEGER PRIMARY KEY,
    project_id INTEGER NOT NULL REFERENCES projects(id),
    ticket_id INTEGER NOT NULL REFERENCES tickets(id),
    submission_id INTEGER NOT NULL REFERENCES submissions(id),
    tip TEXT NOT NULL,
    configuration_version INTEGER NOT NULL,
    priority TEXT NOT NULL,
    version INTEGER NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('in_progress','approved','rejected'))
);
CREATE UNIQUE INDEX active_review_ticket ON review_executions(ticket_id) WHERE status='in_progress';
CREATE TABLE review_slots (
    id INTEGER PRIMARY KEY,
    review_id INTEGER NOT NULL REFERENCES review_executions(id),
    stage_index INTEGER NOT NULL,
    slot_index INTEGER NOT NULL,
    snapshot TEXT NOT NULL CHECK(json_valid(snapshot)),
    dispatch_request_id INTEGER REFERENCES dispatch_requests(id),
    UNIQUE(review_id,stage_index,slot_index)
);
CREATE TABLE review_slot_verdicts (
    slot_id INTEGER PRIMARY KEY REFERENCES review_slots(id),
    record TEXT NOT NULL CHECK(json_valid(record))
);
CREATE TRIGGER review_verdict_no_update BEFORE UPDATE ON review_slot_verdicts BEGIN SELECT RAISE(ABORT,'review verdicts are immutable'); END;
CREATE TRIGGER review_verdict_no_delete BEFORE DELETE ON review_slot_verdicts BEGIN SELECT RAISE(ABORT,'review verdicts are immutable'); END;
ALTER TABLE dispatch_requests ADD COLUMN reviewer_slot_id INTEGER REFERENCES review_slots(id);
ALTER TABLE dispatch_requests ADD COLUMN completed_at INTEGER;
DROP INDEX dispatch_requests_open_ticket;
CREATE UNIQUE INDEX dispatch_requests_open_ticket ON dispatch_requests(ticket_id)
WHERE status IN ('queued','claimed') AND completed_at IS NULL AND reviewer_slot_id IS NULL;
CREATE UNIQUE INDEX dispatch_requests_review_slot ON dispatch_requests(reviewer_slot_id)
WHERE reviewer_slot_id IS NOT NULL;
