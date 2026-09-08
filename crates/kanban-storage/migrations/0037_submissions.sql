-- Immutable structured results are the authority, not agent output.
CREATE TABLE submissions (
    id INTEGER PRIMARY KEY,
    project_id INTEGER NOT NULL REFERENCES projects(id),
    ticket_id INTEGER NOT NULL REFERENCES tickets(id),
    run_id INTEGER NOT NULL UNIQUE REFERENCES runs(id),
    capability_id INTEGER NOT NULL REFERENCES capabilities(id),
    record TEXT NOT NULL CHECK (json_valid(record))
);
CREATE TRIGGER submissions_no_update BEFORE UPDATE ON submissions
BEGIN SELECT RAISE(ABORT, 'submissions are append-only'); END;
CREATE TRIGGER submissions_no_delete BEFORE DELETE ON submissions
BEGIN SELECT RAISE(ABORT, 'submissions are append-only'); END;
CREATE TRIGGER submissions_no_replace BEFORE INSERT ON submissions
WHEN EXISTS (SELECT 1 FROM submissions WHERE id = NEW.id OR run_id = NEW.run_id)
BEGIN SELECT RAISE(ABORT, 'submissions are append-only'); END;
