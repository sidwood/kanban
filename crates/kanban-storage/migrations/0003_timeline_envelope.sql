-- 0003 timeline envelope
--
-- One append-only timeline per Project: every row carries the
-- project identity and optional entity reference the query surface
-- filters on (KAN-S2-US1).

ALTER TABLE timeline_events ADD COLUMN project_id TEXT NOT NULL DEFAULT '';
ALTER TABLE timeline_events ADD COLUMN entity_kind TEXT;
ALTER TABLE timeline_events ADD COLUMN entity_id TEXT;

CREATE INDEX idx_timeline_events_project_recorded
    ON timeline_events (project_id, recorded_at);

CREATE INDEX idx_timeline_events_project_entity
    ON timeline_events (project_id, entity_kind, entity_id);

CREATE INDEX idx_timeline_events_project_kind
    ON timeline_events (project_id, kind);
