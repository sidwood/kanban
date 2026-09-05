-- 0011 supersession uniqueness
--
-- One non-null successor per original for rulings and deferrals
-- (KAN-S2-US3, DR-AE-03). Supersession forms a single unambiguous
-- chain; a second successor for the same original is refused.

CREATE UNIQUE INDEX idx_rulings_supersedes_unique
    ON rulings (supersedes_id)
    WHERE supersedes_id IS NOT NULL;

CREATE UNIQUE INDEX idx_deferrals_supersedes_unique
    ON deferrals (supersedes_id)
    WHERE supersedes_id IS NOT NULL;
