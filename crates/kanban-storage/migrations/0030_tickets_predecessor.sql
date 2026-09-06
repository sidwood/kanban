-- 0030 tickets predecessor
--
-- Reassignment by replacement (KAN-T22, DR-DE-07): a replacement
-- Ticket references the Ticket it replaces. The `tickets` table gains
-- the nullable `predecessor_id`, set once by the reassignment write
-- that creates the replacement and immutable afterwards; ordinary
-- Tickets carry none. The reference stays one-directional — the
-- superseded original records its replacement on the timeline alone —
-- and no row is ever rewritten or deleted, so the superseded original
-- keeps its history and its minted number stays unique (DR-AE-09).

ALTER TABLE tickets ADD COLUMN predecessor_id INTEGER REFERENCES tickets (id);
