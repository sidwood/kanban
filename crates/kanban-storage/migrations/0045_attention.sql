CREATE TABLE attention_items (
    id TEXT PRIMARY KEY,
    project_id INTEGER NOT NULL REFERENCES projects(id),
    kind TEXT NOT NULL CHECK (kind IN ('blocker','missing_result','human_decision','review_request','failed_schedule','invalid_approval','disconnected_session','stale_run')),
    subject_kind TEXT NOT NULL CHECK (subject_kind IN ('ticket','run','spec','project','deferral','graph','schedule','role')),
    subject_id TEXT NOT NULL,
    summary TEXT NOT NULL,
    detail TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    version INTEGER NOT NULL CHECK (version > 0),
    active INTEGER NOT NULL CHECK (active IN (0,1)),
    acknowledged_by TEXT,
    acknowledged_at TEXT,
    first_seen_at TEXT NOT NULL,
    last_seen_at TEXT NOT NULL,
    CHECK ((acknowledged_by IS NULL) = (acknowledged_at IS NULL))
);
CREATE INDEX attention_items_current ON attention_items(active,acknowledged_by,project_id,kind);

CREATE TABLE attention_acknowledgements (
    id INTEGER PRIMARY KEY,
    item_id TEXT NOT NULL REFERENCES attention_items(id),
    item_version INTEGER NOT NULL,
    who TEXT NOT NULL CHECK (length(trim(who)) > 0),
    acknowledged_at TEXT NOT NULL,
    snapshot TEXT NOT NULL,
    UNIQUE(item_id,item_version)
);
CREATE TRIGGER attention_ack_no_update BEFORE UPDATE ON attention_acknowledgements
BEGIN SELECT RAISE(ABORT,'acknowledgement history is immutable'); END;
CREATE TRIGGER attention_ack_no_delete BEFORE DELETE ON attention_acknowledgements
BEGIN SELECT RAISE(ABORT,'acknowledgement history is retained'); END;
CREATE TRIGGER attention_ack_no_replace BEFORE INSERT ON attention_acknowledgements
WHEN EXISTS(SELECT 1 FROM attention_acknowledgements WHERE id=NEW.id OR (item_id=NEW.item_id AND item_version=NEW.item_version))
BEGIN SELECT RAISE(ABORT,'acknowledgement history cannot be replaced'); END;

CREATE TABLE schedule_failures (
    schedule_id INTEGER PRIMARY KEY REFERENCES schedules(id),
    project_id INTEGER NOT NULL REFERENCES projects(id),
    ticket_id INTEGER NOT NULL REFERENCES tickets(id),
    error_code TEXT NOT NULL,
    failed_at TEXT NOT NULL,
    last_seen_at TEXT NOT NULL
);

CREATE TABLE notification_settings (
    project_id INTEGER PRIMARY KEY REFERENCES projects(id),
    local_enabled INTEGER NOT NULL CHECK (local_enabled IN (0,1)),
    mirror_role TEXT CHECK (mirror_role IS NULL OR length(trim(mirror_role)) > 0),
    version INTEGER NOT NULL CHECK (version > 0)
);

CREATE TABLE notification_deliveries (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id INTEGER NOT NULL REFERENCES projects(id),
    item_id TEXT NOT NULL REFERENCES attention_items(id),
    item_version INTEGER NOT NULL CHECK(item_version > 0),
    settings_version INTEGER NOT NULL CHECK(settings_version > 0),
    channel TEXT NOT NULL CHECK(channel IN ('local','herdr_mirror')),
    target TEXT NOT NULL CHECK(json_valid(target)),
    status TEXT NOT NULL CHECK(status IN ('queued','prepared','submitted','failed','uncertain')),
    receipt TEXT,
    last_error TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    version INTEGER NOT NULL DEFAULT 1 CHECK(version > 0),
    UNIQUE(item_id,item_version,channel)
);

CREATE TABLE observed_attention_signals (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id INTEGER NOT NULL REFERENCES projects(id),
    source_key TEXT NOT NULL UNIQUE,
    reason TEXT NOT NULL CHECK(reason IN ('missing_submission','missing_result_deadline_breached','stall_deadline_breached')),
    detail TEXT NOT NULL CHECK(json_valid(detail)),
    observed_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE TABLE observed_attention_connections (
    project_id INTEGER PRIMARY KEY REFERENCES projects(id),
    binding TEXT NOT NULL,
    diagnostics TEXT NOT NULL CHECK(json_valid(diagnostics))
);
