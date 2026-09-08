CREATE TABLE finding_promotions (
    project_id INTEGER NOT NULL REFERENCES projects(id),
    finding_id TEXT NOT NULL,
    deferral_id INTEGER NOT NULL UNIQUE REFERENCES deferrals(id),
    ticket_id INTEGER NOT NULL UNIQUE REFERENCES tickets(id),
    PRIMARY KEY(project_id,finding_id)
);
CREATE TRIGGER finding_promotion_no_update BEFORE UPDATE ON finding_promotions
BEGIN SELECT RAISE(ABORT,'finding promotions are immutable'); END;
CREATE TRIGGER finding_promotion_no_delete BEFORE DELETE ON finding_promotions
BEGIN SELECT RAISE(ABORT,'finding promotions are immutable'); END;
CREATE TRIGGER finding_promotion_no_replace BEFORE INSERT ON finding_promotions
WHEN EXISTS(SELECT 1 FROM finding_promotions WHERE deferral_id=NEW.deferral_id OR ticket_id=NEW.ticket_id OR (project_id=NEW.project_id AND finding_id=NEW.finding_id))
BEGIN SELECT RAISE(ABORT,'finding already promoted'); END;

-- A stable finding reference must never name a replacement verdict.
CREATE TRIGGER review_verdict_no_replace BEFORE INSERT ON review_slot_verdicts
WHEN EXISTS(SELECT 1 FROM review_slot_verdicts WHERE slot_id=NEW.slot_id)
BEGIN SELECT RAISE(ABORT,'review verdicts are immutable'); END;

-- REPLACE can bypass delete triggers when recursive triggers are disabled.
-- Refuse both identity replacement and replacement through a successor index.
CREATE TRIGGER deferrals_append_only_insert BEFORE INSERT ON deferrals
WHEN EXISTS(SELECT 1 FROM deferrals WHERE id=NEW.id OR (NEW.supersedes_id IS NOT NULL AND supersedes_id=NEW.supersedes_id))
BEGIN SELECT RAISE(ABORT,'deferrals is append-only'); END;
CREATE TRIGGER rulings_append_only_insert BEFORE INSERT ON rulings
WHEN EXISTS(SELECT 1 FROM rulings WHERE id=NEW.id OR (NEW.supersedes_id IS NOT NULL AND supersedes_id=NEW.supersedes_id))
BEGIN SELECT RAISE(ABORT,'rulings is append-only'); END;
