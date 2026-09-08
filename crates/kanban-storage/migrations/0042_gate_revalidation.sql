-- Failed or expired gates stay on the ticket until revalidation.
CREATE TABLE review_executions_v42 (
    id INTEGER PRIMARY KEY,
    project_id INTEGER NOT NULL REFERENCES projects(id),
    ticket_id INTEGER NOT NULL REFERENCES tickets(id),
    submission_id INTEGER NOT NULL REFERENCES submissions(id),
    tip TEXT NOT NULL,
    configuration_version INTEGER NOT NULL,
    priority TEXT NOT NULL,
    version INTEGER NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('in_progress','approved','rejected','expired'))
);
INSERT INTO review_executions_v42 SELECT * FROM review_executions;
DROP TABLE review_executions;
ALTER TABLE review_executions_v42 RENAME TO review_executions;
CREATE UNIQUE INDEX active_review_ticket ON review_executions(ticket_id) WHERE status='in_progress';

CREATE TABLE review_gates (
    ticket_id INTEGER PRIMARY KEY REFERENCES tickets(id),
    needs_revalidation INTEGER NOT NULL DEFAULT 0 CHECK(needs_revalidation IN (0,1)),
    latest_review_id INTEGER REFERENCES review_executions(id)
);
CREATE TABLE review_execution_attempts (
    id INTEGER PRIMARY KEY,
    ticket_id INTEGER NOT NULL REFERENCES tickets(id),
    review_id INTEGER REFERENCES review_executions(id),
    attempt INTEGER NOT NULL,
    outcome TEXT NOT NULL CHECK(outcome IN ('failed','expired','approved','open')),
    verdicts TEXT NOT NULL CHECK(json_valid(verdicts)),
    invalidations TEXT NOT NULL CHECK(json_valid(invalidations)),
    UNIQUE(ticket_id, attempt)
);
CREATE TRIGGER review_attempts_no_delete BEFORE DELETE ON review_execution_attempts
BEGIN SELECT RAISE(ABORT,'review attempts are immutable'); END;
CREATE TRIGGER review_attempts_no_replace BEFORE UPDATE ON review_execution_attempts
BEGIN SELECT RAISE(ABORT,'review attempts are immutable'); END;
