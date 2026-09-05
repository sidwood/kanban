-- 0015 project Herdr workspace binding
--
-- Version 2 of KAN-S1 and KAN-S8 replaces the exclusive named-session
-- contract: a Project stores one required target Herdr workspace and
-- an optional session whose absence selects Herdr's default session
-- (DR-PH-07, DR-HB-18). Session names stop being exclusive, so the
-- rebuild drops that uniqueness rule; codes stay globally unique and
-- Projects stay archived, never deleted.
--
-- Every existing Project keeps its session name and takes the Herdr
-- workspace identity its Seed Workspace was already observed through:
-- the Seed's final path segment, the same identity the session
-- snapshots report. The application is pre-release, so this is normal
-- schema discipline, not a fleet migration.
--
-- The rebuild drops the projects table while Herdr settings, Plans,
-- and Workspaces still reference it; the migration runner applies
-- schema changes with foreign key enforcement off and validates the
-- final state with a foreign key check before committing.

CREATE TABLE projects_rebound (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    code            TEXT NOT NULL UNIQUE,
    name            TEXT NOT NULL,
    repository      TEXT NOT NULL,
    seed_workspace  TEXT NOT NULL,
    default_branch  TEXT NOT NULL,
    herdr_session   TEXT,
    herdr_workspace TEXT NOT NULL,
    initiative_id   INTEGER REFERENCES initiatives (id),
    archived        INTEGER NOT NULL DEFAULT 0 CHECK (archived IN (0, 1)),
    plan_counter    INTEGER NOT NULL DEFAULT 0 CHECK (plan_counter >= 0),
    spec_counter    INTEGER NOT NULL DEFAULT 0 CHECK (spec_counter >= 0),
    ticket_counter  INTEGER NOT NULL DEFAULT 0 CHECK (ticket_counter >= 0),
    version         INTEGER NOT NULL CHECK (version > 0),
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    archived_at     TEXT
);

INSERT INTO projects_rebound
    (id, code, name, repository, seed_workspace, default_branch,
     herdr_session, herdr_workspace, initiative_id, archived,
     plan_counter, spec_counter, ticket_counter, version,
     created_at, archived_at)
SELECT id, code, name, repository, seed_workspace, default_branch,
       herdr_session,
       COALESCE(
           NULLIF(
               substr(rtrim(seed_workspace, '/'),
                      length(rtrim(rtrim(seed_workspace, '/'),
                                   replace(rtrim(seed_workspace, '/'), '/', ''))) + 1),
               ''),
           NULLIF(rtrim(seed_workspace, '/'), ''),
           seed_workspace),
       initiative_id, archived, plan_counter, spec_counter,
       ticket_counter, version, created_at, archived_at
FROM projects;

-- The delete-refusal trigger would abort the table rebuild's implicit
-- row deletes, so it drops first and returns unchanged afterwards.
DROP TRIGGER projects_refuse_delete;

DROP TABLE projects;

ALTER TABLE projects_rebound RENAME TO projects;

CREATE TRIGGER projects_refuse_delete
BEFORE DELETE ON projects
BEGIN
    SELECT RAISE(ABORT, 'projects are archived, never deleted');
END;
