CREATE TABLE task_occurrences (
    id INTEGER PRIMARY KEY,
    schedule_id INTEGER NOT NULL REFERENCES schedules(id),
    template_ticket_id INTEGER NOT NULL REFERENCES tickets(id),
    occurrence_ticket_id INTEGER NOT NULL UNIQUE REFERENCES tickets(id),
    window_at TEXT NOT NULL,
    CHECK (template_ticket_id <> occurrence_ticket_id),
    UNIQUE (template_ticket_id, window_at)
);

CREATE TRIGGER task_occurrence_lineage_immutable
BEFORE UPDATE ON task_occurrences
BEGIN
    SELECT RAISE(ABORT, 'occurrence lineage is immutable');
END;

CREATE TABLE project_schedule_policies (
    project_id INTEGER PRIMARY KEY REFERENCES projects(id),
    catch_up_one INTEGER NOT NULL CHECK (catch_up_one IN (0,1)),
    version INTEGER NOT NULL CHECK (version > 0)
);

CREATE TABLE schedule_attention (
    id INTEGER PRIMARY KEY,
    project_id INTEGER NOT NULL REFERENCES projects(id),
    template_ticket_id INTEGER NOT NULL REFERENCES tickets(id),
    schedule_id INTEGER NOT NULL REFERENCES schedules(id),
    reason TEXT NOT NULL CHECK (reason IN ('missed_window','overlap','blocked')),
    first_window TEXT NOT NULL,
    last_window TEXT NOT NULL,
    next_activation TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE (template_ticket_id, reason)
);
CREATE INDEX schedule_attention_project ON schedule_attention(project_id, updated_at);

CREATE TRIGGER task_occurrence_lineage_retained
BEFORE DELETE ON task_occurrences
BEGIN
    SELECT RAISE(ABORT, 'occurrence lineage is retained');
END;

CREATE TRIGGER task_occurrence_no_replacement
BEFORE INSERT ON task_occurrences
WHEN EXISTS (
    SELECT 1 FROM task_occurrences
    WHERE id = NEW.id OR occurrence_ticket_id = NEW.occurrence_ticket_id
       OR (template_ticket_id = NEW.template_ticket_id AND window_at = NEW.window_at)
)
BEGIN
    SELECT RAISE(ABORT, 'occurrence lineage cannot be replaced');
END;
