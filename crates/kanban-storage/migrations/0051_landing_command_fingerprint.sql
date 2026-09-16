-- 0051 landing command fingerprint
--
-- Reconcile must resolve the original landing command identity as well
-- as the Workspace reservation (KAN-T146-AC2). The fingerprint observed
-- at prepare is what a completed recovery writes into
-- idempotency_outcomes so the original key can replay.

ALTER TABLE landing_intents ADD COLUMN command_fingerprint TEXT NOT NULL DEFAULT '';
