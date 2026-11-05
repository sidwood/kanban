-- Indexed canonical comment targets.
--
-- Listing matches a comment by target_key, not by scanning every row
-- of the same kind. Numeric spellings collapse to the ticket id. Key
-- spellings collapse to the project code and issue number.

ALTER TABLE comments ADD COLUMN target_key TEXT
GENERATED ALWAYS AS (
    CASE
        WHEN entity_kind = 'ticket'
         AND (
             entity_id GLOB '+[0-9]*'
             OR (
                 entity_id GLOB '[0-9]*'
                 AND entity_id NOT GLOB '*[^0-9]*'
             )
         )
            THEN CAST(CAST(entity_id AS INTEGER) AS TEXT)
        WHEN entity_kind = 'ticket'
         AND instr(entity_id, '-') > 1
         AND length(substr(entity_id, 1, instr(entity_id, '-') - 1)) BETWEEN 2 AND 8
         AND substr(entity_id, 1, 1) GLOB '[A-Za-z]'
         AND substr(entity_id, 1, instr(entity_id, '-') - 1) NOT GLOB '*[^A-Za-z0-9]*'
         AND substr(entity_id, instr(entity_id, '-') + 1) GLOB '[0-9]*'
         AND substr(entity_id, instr(entity_id, '-') + 1) NOT GLOB '*[^0-9]*'
         AND CAST(substr(entity_id, instr(entity_id, '-') + 1) AS INTEGER) > 0
            THEN UPPER(substr(entity_id, 1, instr(entity_id, '-') - 1))
                 || '-'
                 || CAST(
                     CAST(substr(entity_id, instr(entity_id, '-') + 1) AS INTEGER)
                     AS TEXT
                 )
        ELSE entity_id
    END
) VIRTUAL;

CREATE INDEX IF NOT EXISTS comments_by_canonical_target
    ON comments (entity_kind, target_key, id);
