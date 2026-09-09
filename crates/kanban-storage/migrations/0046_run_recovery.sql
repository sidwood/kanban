CREATE TABLE run_recoveries (
    id INTEGER PRIMARY KEY,
    run_id INTEGER NOT NULL REFERENCES runs(id),
    ruling_id INTEGER NOT NULL REFERENCES rulings(id),
    action TEXT NOT NULL DEFAULT 'operator_ruling' CHECK (action IN ('operator_ruling', 'retry', 'resume')),
    replacement_dispatch_request_id INTEGER REFERENCES dispatch_requests(id),
    sequence INTEGER NOT NULL CHECK (sequence > 0),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()) CHECK (created_at >= 0),
    UNIQUE (run_id, sequence),
    UNIQUE (ruling_id),
    CHECK((action='retry' AND replacement_dispatch_request_id IS NOT NULL)
       OR (action IN ('operator_ruling','resume') AND replacement_dispatch_request_id IS NULL))
);
CREATE UNIQUE INDEX run_recoveries_one_retry ON run_recoveries(run_id) WHERE action='retry';
DROP INDEX dispatch_requests_review_slot;
CREATE UNIQUE INDEX dispatch_requests_review_slot ON dispatch_requests(reviewer_slot_id)
WHERE reviewer_slot_id IS NOT NULL AND completed_at IS NULL;

CREATE TRIGGER run_recoveries_no_update
BEFORE UPDATE ON run_recoveries BEGIN
    SELECT RAISE(ABORT, 'run recovery history is immutable');
END;

CREATE TRIGGER run_recoveries_no_delete
BEFORE DELETE ON run_recoveries BEGIN
    SELECT RAISE(ABORT, 'run recovery history is immutable');
END;
