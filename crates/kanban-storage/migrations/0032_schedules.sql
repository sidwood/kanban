-- 0032 schedules
--
-- One Schedule row per scheduled activation (KAN-T53): a one-time
-- Schedule holds the existing Ticket it will make ready, carrying
-- its activation instant, its timezone, its eligible Execution
-- Profile, and its next activation (DR-SA-01). The closed trigger
-- vocabulary keeps one_time and cron rows to exactly their own
-- columns — recurring rows arrive with KAN-T54 — and a one-time
-- row's next activation is its activation. The waiting/fired state
-- keeps a fired one-time Schedule from ever firing again (DR-SA-06)
-- and gives the due scan its index; firing and the overdue restart
-- pass live in the core service, not the schema.

CREATE TABLE schedules (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    ticket_id       INTEGER NOT NULL REFERENCES tickets (id),
    trigger_kind    TEXT NOT NULL CHECK (trigger_kind IN ('one_time', 'cron')),
    activation_at   TEXT,
    cron_expression TEXT,
    timezone        TEXT NOT NULL CHECK (length(trim(timezone)) > 0),
    profile         TEXT NOT NULL CHECK (length(trim(profile)) > 0),
    next_activation TEXT NOT NULL,
    state           TEXT NOT NULL DEFAULT 'waiting'
                    CHECK (state IN ('waiting', 'fired')),
    fired_at        TEXT,
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    version         INTEGER NOT NULL DEFAULT 1 CHECK (version > 0),
    CHECK (
        (trigger_kind = 'one_time'
            AND activation_at IS NOT NULL
            AND cron_expression IS NULL)
        OR
        (trigger_kind = 'cron'
            AND cron_expression IS NOT NULL)
    ),
    CHECK (trigger_kind <> 'one_time' OR activation_at = next_activation),
    CHECK (state <> 'fired' OR fired_at IS NOT NULL)
);

CREATE INDEX schedules_due
    ON schedules (state, trigger_kind, next_activation);
