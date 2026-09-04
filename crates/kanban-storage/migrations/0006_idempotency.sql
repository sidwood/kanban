-- 0006 idempotency outcomes
--
-- A successful mutation and the outcome that replays it commit in
-- one transaction (DR-SS-03, KAN-S1-US2), so no crash boundary can
-- leave a mutation applied with no outcome for its retry to find. A
-- key is spent once and its outcome never rewritten; pruning the
-- oldest beyond the retained bound is the only delete.

CREATE TABLE idempotency_outcomes (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    idempotency_key TEXT NOT NULL UNIQUE,
    fingerprint     TEXT NOT NULL,
    response        TEXT NOT NULL,
    recorded_at     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TRIGGER idempotency_outcomes_write_once
BEFORE UPDATE ON idempotency_outcomes
BEGIN
    SELECT RAISE(ABORT, 'idempotency_outcomes is write-once');
END;
