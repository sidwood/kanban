CREATE TABLE run_resume_deliveries (
    recovery_id INTEGER PRIMARY KEY REFERENCES run_recoveries(id),
    status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'delivered', 'obsolete')),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    next_attempt_at INTEGER NOT NULL DEFAULT 0 CHECK (next_attempt_at >= 0)
);
CREATE INDEX run_resume_deliveries_pending ON run_resume_deliveries(status, next_attempt_at);
INSERT INTO run_resume_deliveries(recovery_id)
SELECT id FROM run_recoveries WHERE action='resume';
