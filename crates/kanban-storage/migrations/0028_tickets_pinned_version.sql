-- 0028 tickets pinned version
--
-- Graph approval pins every Ticket to the Spec content version its
-- approved graph named (DR-DE-06, KAN-T23). The pin is a plain
-- column on `tickets`: NULL while the Ticket moves freely among its
-- Project's Specs, the approved version once a graph approval set
-- it, and never moved again. The proposal rows themselves land with
-- their own migration.

ALTER TABLE tickets ADD COLUMN pinned_version INTEGER
    CHECK (pinned_version IS NULL OR pinned_version > 0);
