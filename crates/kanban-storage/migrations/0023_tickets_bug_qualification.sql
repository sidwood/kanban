-- 0023 tickets bug qualification
--
-- Bug capture and qualification (KAN-T18). Four columns join
-- `tickets`: the quick-capture facts every Bug is created with
-- (DR-TK-08), the qualification that completes it as one JSON record
-- (DR-TK-09), and the vendor-neutral collections it carries while it
-- waits (DR-TK-10). Rows written by 0020 named none of these, so the
-- columns arrive nullable and triggers — the same schema-level
-- strengthening pattern 0012 set — keep every new write to each
-- kind's own fields: a Bug carries its capture facts and its facts
-- blob, no other kind carries either, and a 0020 Bug row simply
-- rehydrates with empty capture text until it is edited.

ALTER TABLE tickets ADD COLUMN actual_behaviour TEXT;
ALTER TABLE tickets ADD COLUMN reporter_evidence TEXT;
ALTER TABLE tickets ADD COLUMN bug_qualification TEXT;
ALTER TABLE tickets ADD COLUMN bug_facts TEXT;

CREATE TRIGGER tickets_bug_shape_insert
BEFORE INSERT ON tickets
WHEN (NEW.kind = 'bug'
        AND (NEW.actual_behaviour IS NULL
            OR NEW.reporter_evidence IS NULL
            OR NEW.bug_facts IS NULL))
    OR (NEW.kind <> 'bug'
        AND (NEW.actual_behaviour IS NOT NULL
            OR NEW.reporter_evidence IS NOT NULL
            OR NEW.bug_qualification IS NOT NULL
            OR NEW.bug_facts IS NOT NULL))
BEGIN
    SELECT RAISE(ABORT, 'each Ticket kind carries exactly its own fields');
END;

CREATE TRIGGER tickets_bug_shape_update
BEFORE UPDATE ON tickets
WHEN (NEW.kind = 'bug'
        AND (NEW.actual_behaviour IS NULL
            OR NEW.reporter_evidence IS NULL
            OR NEW.bug_facts IS NULL))
    OR (NEW.kind <> 'bug'
        AND (NEW.actual_behaviour IS NOT NULL
            OR NEW.reporter_evidence IS NOT NULL
            OR NEW.bug_qualification IS NOT NULL
            OR NEW.bug_facts IS NOT NULL))
BEGIN
    SELECT RAISE(ABORT, 'each Ticket kind carries exactly its own fields');
END;
