//! Idempotent application of migration 53 for recovery re-runs.

use rusqlite::{Connection, OptionalExtension};

use crate::db::WriteSpan;

fn table_has_column(conn: &Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
    let mut statement = conn.prepare(&format!(
        "SELECT 1 FROM pragma_table_info('{table}') WHERE name = ?1"
    ))?;
    let found = statement
        .query_row([column], |_| Ok(()))
        .optional()?
        .is_some();
    Ok(found)
}

fn add_column(
    conn: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> rusqlite::Result<()> {
    if table_has_column(conn, table, column)? {
        return Ok(());
    }
    conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {definition}"))
}

/// Applies the linear issue model once; safe when tables or columns already exist.
pub(crate) fn apply_linear_issue_model(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS issue_workflow_states (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            project_id    INTEGER NOT NULL REFERENCES projects (id),
            name          TEXT NOT NULL,
            type          TEXT NOT NULL CHECK (type IN (
                'backlog', 'unstarted', 'started', 'completed', 'canceled', 'duplicate'
            )),
            legacy_state  TEXT NOT NULL,
            position      INTEGER NOT NULL CHECK (position >= 0),
            UNIQUE (project_id, name),
            UNIQUE (project_id, legacy_state)
        );
        CREATE TABLE IF NOT EXISTS issue_label_groups (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            project_id    INTEGER NOT NULL REFERENCES projects (id),
            name          TEXT NOT NULL,
            single_select INTEGER NOT NULL DEFAULT 0 CHECK (single_select IN (0, 1)),
            UNIQUE (project_id, name)
        );
        CREATE TABLE IF NOT EXISTS issue_labels (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            project_id  INTEGER NOT NULL REFERENCES projects (id),
            group_id    INTEGER REFERENCES issue_label_groups (id),
            name        TEXT NOT NULL,
            color       TEXT NOT NULL DEFAULT '#9CA3AF',
            retired_at  TEXT,
            UNIQUE (project_id, name)
        );
        CREATE TABLE IF NOT EXISTS issue_ticket_labels (
            ticket_id INTEGER NOT NULL REFERENCES tickets (id),
            label_id  INTEGER NOT NULL REFERENCES issue_labels (id),
            PRIMARY KEY (ticket_id, label_id)
        );
        CREATE TABLE IF NOT EXISTS issue_milestones (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            project_id  INTEGER NOT NULL REFERENCES projects (id),
            name        TEXT NOT NULL,
            sort_order  REAL NOT NULL DEFAULT 0,
            target_date TEXT,
            UNIQUE (project_id, name)
        );
        CREATE TABLE IF NOT EXISTS issue_state_history (
            id                 INTEGER PRIMARY KEY AUTOINCREMENT,
            ticket_id          INTEGER NOT NULL REFERENCES tickets (id),
            workflow_state_id  INTEGER NOT NULL REFERENCES issue_workflow_states (id),
            started_at         TEXT NOT NULL,
            ended_at           TEXT
        );
        CREATE TABLE IF NOT EXISTS issue_relations (
            id             INTEGER PRIMARY KEY AUTOINCREMENT,
            from_ticket_id INTEGER NOT NULL REFERENCES tickets (id),
            to_ticket_id   INTEGER NOT NULL REFERENCES tickets (id),
            kind           TEXT NOT NULL CHECK (kind IN ('blocks', 'related', 'duplicate')),
            CHECK (from_ticket_id <> to_ticket_id),
            UNIQUE (from_ticket_id, to_ticket_id, kind)
        );",
    )?;

    add_column(
        conn,
        "tickets",
        "workflow_state_id",
        "workflow_state_id INTEGER REFERENCES issue_workflow_states (id)",
    )?;
    add_column(
        conn,
        "tickets",
        "linear_priority",
        "linear_priority INTEGER NOT NULL DEFAULT 3 CHECK (linear_priority BETWEEN 0 AND 4)",
    )?;
    add_column(
        conn,
        "tickets",
        "due_date",
        "due_date TEXT NOT NULL DEFAULT '2099-12-31'",
    )?;
    add_column(conn, "tickets", "estimate", "estimate REAL")?;
    add_column(
        conn,
        "tickets",
        "milestone_id",
        "milestone_id INTEGER REFERENCES issue_milestones (id)",
    )?;
    add_column(conn, "tickets", "started_at", "started_at TEXT")?;
    add_column(conn, "tickets", "completed_at", "completed_at TEXT")?;
    add_column(conn, "tickets", "canceled_at", "canceled_at TEXT")?;
    add_column(
        conn,
        "tickets",
        "duplicate_of_ticket_id",
        "duplicate_of_ticket_id INTEGER REFERENCES tickets (id)",
    )?;
    add_column(
        conn,
        "comments",
        "parent_comment_id",
        "parent_comment_id INTEGER REFERENCES comments (id)",
    )?;
    add_column(conn, "comments", "resolved_at", "resolved_at TEXT")?;

    conn.execute_batch(
        "INSERT OR IGNORE INTO issue_workflow_states (project_id, name, type, legacy_state, position)
        SELECT p.id, v.name, v.type, v.legacy_state, v.position
        FROM projects AS p
        CROSS JOIN (
            SELECT 'Draft' AS name, 'backlog' AS type, 'draft' AS legacy_state, 0 AS position
            UNION ALL SELECT 'Parked', 'backlog', 'parked', 1
            UNION ALL SELECT 'Blocked', 'backlog', 'blocked', 2
            UNION ALL SELECT 'Scheduled', 'unstarted', 'scheduled', 3
            UNION ALL SELECT 'Ready', 'unstarted', 'ready', 4
            UNION ALL SELECT 'Active', 'started', 'active', 5
            UNION ALL SELECT 'In Review', 'started', 'in_review', 6
            UNION ALL SELECT 'Approved', 'started', 'approved', 7
            UNION ALL SELECT 'Landing', 'started', 'landing', 8
            UNION ALL SELECT 'Done', 'completed', 'done', 9
            UNION ALL SELECT 'Cancelled', 'canceled', 'cancelled', 10
            UNION ALL SELECT 'Superseded', 'canceled', 'superseded', 11
        ) AS v;
        INSERT OR IGNORE INTO issue_label_groups (project_id, name, single_select)
        SELECT id, 'Type', 1 FROM projects;
        INSERT OR IGNORE INTO issue_labels (project_id, group_id, name, color)
        SELECT p.id, g.id, v.name, v.color
        FROM projects AS p
        JOIN issue_label_groups AS g ON g.project_id = p.id AND g.name = 'Type'
        CROSS JOIN (
            SELECT 'implementation' AS name, '#6366F1' AS color
            UNION ALL SELECT 'bug', '#EF4444'
            UNION ALL SELECT 'task', '#10B981'
        ) AS v;
        INSERT OR IGNORE INTO issue_ticket_labels (ticket_id, label_id)
        SELECT t.id, l.id
        FROM tickets AS t
        JOIN issue_labels AS l
          ON l.project_id = t.project_id AND l.name = t.kind;
        UPDATE tickets
        SET
            actual_behaviour = COALESCE(actual_behaviour, ''),
            reporter_evidence = COALESCE(reporter_evidence, ''),
            bug_facts = COALESCE(bug_facts, '{}')
        WHERE kind = 'bug';
        UPDATE tickets
        SET workflow_state_id = (
            SELECT s.id FROM issue_workflow_states AS s
            WHERE s.project_id = tickets.project_id
              AND s.legacy_state = tickets.state
        )
        WHERE workflow_state_id IS NULL;
        UPDATE tickets
        SET linear_priority = CASE priority
            WHEN 'urgent' THEN 1
            WHEN 'high' THEN 2
            WHEN 'normal' THEN 3
            WHEN 'low' THEN 4
            ELSE 0
        END;
        INSERT INTO issue_state_history (ticket_id, workflow_state_id, started_at, ended_at)
        SELECT t.id, t.workflow_state_id, t.created_at, NULL
        FROM tickets AS t
        WHERE t.workflow_state_id IS NOT NULL
          AND NOT EXISTS (
              SELECT 1 FROM issue_state_history AS h
              WHERE h.ticket_id = t.id AND h.ended_at IS NULL
          );
        INSERT OR IGNORE INTO issue_relations (from_ticket_id, to_ticket_id, kind)
        SELECT from_ticket, to_ticket, 'blocks'
        FROM ticket_dependencies;",
    )?;

    Ok(())
}

pub(crate) fn apply_linear_issue_model_in_span(span: &WriteSpan<'_>) -> rusqlite::Result<()> {
    apply_linear_issue_model(span)
}

fn tickets_table_has_column(conn: &Connection, column: &str) -> rusqlite::Result<bool> {
    let mut statement =
        conn.prepare("SELECT 1 FROM pragma_table_info('tickets') WHERE name = ?1")?;
    Ok(statement
        .query_row([column], |_| Ok(()))
        .optional()?
        .is_some())
}

/// Relaxes kind shape triggers/CHECK and adds issue description metadata.
pub(crate) fn apply_linear_issue_relaxation(conn: &Connection) -> rusqlite::Result<()> {
    if tickets_table_has_column(conn, "description")? {
        conn.execute_batch(
            "UPDATE tickets
             SET due_date = due
             WHERE due IS NOT NULL
               AND due <> ''
               AND due_date = '2099-12-31';
             DROP TRIGGER IF EXISTS tickets_bug_shape_insert;
             DROP TRIGGER IF EXISTS tickets_bug_shape_update;
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
             END;",
        )?;
        return Ok(());
    }
    conn.execute_batch(
        "DROP TRIGGER IF EXISTS tickets_bug_shape_insert;
         DROP TRIGGER IF EXISTS tickets_bug_shape_update;
         CREATE TABLE tickets_relaxed (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             project_id INTEGER NOT NULL REFERENCES projects (id),
             number INTEGER NOT NULL CHECK (number > 0),
             kind TEXT NOT NULL CHECK (kind IN ('implementation', 'bug', 'task')),
             priority TEXT NOT NULL CHECK (priority IN ('urgent', 'high', 'normal', 'low')),
             state TEXT NOT NULL CHECK (state IN (
                 'draft', 'parked', 'blocked', 'scheduled', 'ready', 'active',
                 'in_review', 'approved', 'landing', 'done', 'cancelled',
                 'superseded'
             )),
             spec_id INTEGER REFERENCES specs (id),
             title TEXT,
             slice TEXT,
             criteria TEXT NOT NULL DEFAULT '[]',
             actual_behaviour TEXT,
             reporter_evidence TEXT,
             bug_qualification TEXT,
             bug_facts TEXT,
             subtype TEXT,
             mode TEXT,
             completion TEXT NOT NULL DEFAULT '[]',
             scheduled_for TEXT,
             due TEXT,
             profile TEXT REFERENCES execution_profiles (name),
             pinned_version INTEGER,
             predecessor_id INTEGER REFERENCES tickets (id),
             workflow_state_id INTEGER REFERENCES issue_workflow_states (id),
             linear_priority INTEGER NOT NULL DEFAULT 3
                 CHECK (linear_priority BETWEEN 0 AND 4),
             due_date TEXT NOT NULL DEFAULT '2099-12-31',
             estimate REAL,
             milestone_id INTEGER REFERENCES issue_milestones (id),
             started_at TEXT,
             completed_at TEXT,
             canceled_at TEXT,
             duplicate_of_ticket_id INTEGER REFERENCES tickets (id),
             description TEXT NOT NULL DEFAULT '',
             updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
             version INTEGER NOT NULL CHECK (version > 0),
             created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
             UNIQUE (project_id, number),
             CHECK (
                 (kind = 'implementation'
                     AND spec_id IS NOT NULL
                     AND slice IS NOT NULL
                     AND title IS NULL
                     AND criteria <> '[]'
                     AND subtype IS NULL
                     AND mode IS NULL
                     AND completion = '[]'
                     AND scheduled_for IS NULL
                     AND due IS NULL)
                 OR (kind = 'bug'
                     AND title IS NOT NULL
                     AND slice IS NULL
                     AND criteria = '[]'
                     AND subtype IS NULL
                     AND mode IS NULL
                     AND completion = '[]'
                     AND scheduled_for IS NULL
                     AND due IS NULL)
                 OR (kind = 'task'
                     AND title IS NOT NULL
                     AND slice IS NULL
                     AND criteria = '[]'
                     AND subtype IN ('operational', 'investigative', 'administrative',
                                     'research', 'prototype', 'migration', 'manual')
                     AND mode IN ('human', 'agent')
                     AND completion <> '[]')
             )
         );
         INSERT INTO tickets_relaxed (
             id, project_id, number, kind, priority, state, spec_id, title, slice,
             criteria, actual_behaviour, reporter_evidence, bug_qualification, bug_facts,
             subtype, mode, completion, scheduled_for, due, profile, pinned_version,
             predecessor_id, workflow_state_id, linear_priority, due_date, estimate,
             milestone_id, started_at, completed_at, canceled_at, duplicate_of_ticket_id,
             description, updated_at, version, created_at
         )
         SELECT
             id, project_id, number, kind, priority, state, spec_id, title, slice,
             criteria, actual_behaviour, reporter_evidence, bug_qualification, bug_facts,
             subtype, mode, completion, scheduled_for, due, profile, pinned_version,
             predecessor_id, workflow_state_id, linear_priority, due_date, estimate,
             milestone_id, started_at, completed_at, canceled_at, duplicate_of_ticket_id,
             '', created_at, version, created_at
         FROM tickets;
         DROP TABLE tickets;
         ALTER TABLE tickets_relaxed RENAME TO tickets;
         CREATE TRIGGER tickets_refuse_delete
         BEFORE DELETE ON tickets
         BEGIN
             SELECT RAISE(ABORT, 'tickets are never deleted; supersede or cancel them');
         END;
         DROP TRIGGER IF EXISTS tickets_bug_shape_insert;
         DROP TRIGGER IF EXISTS tickets_bug_shape_update;
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
         UPDATE tickets
         SET due_date = due
         WHERE due IS NOT NULL
           AND due <> ''
           AND due_date = '2099-12-31';",
    )?;
    Ok(())
}

pub(crate) fn apply_linear_issue_relaxation_in_span(span: &WriteSpan<'_>) -> rusqlite::Result<()> {
    apply_linear_issue_relaxation(span)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrations;

    #[test]
    fn migration_0053_reapply_is_idempotent() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("kanban.sqlite");
        let conn = rusqlite::Connection::open(&path).unwrap();
        migrations::apply_through(&conn, 52).unwrap();
        apply_linear_issue_model(&conn).unwrap();
        apply_linear_issue_model(&conn).unwrap();
    }

    #[test]
    fn migration_0054_reapply_is_idempotent() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("kanban.sqlite");
        let conn = rusqlite::Connection::open(&path).unwrap();
        migrations::apply_through(&conn, 53).unwrap();
        apply_linear_issue_relaxation(&conn).unwrap();
        apply_linear_issue_relaxation(&conn).unwrap();
        let columns: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('tickets') WHERE name = 'description'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(columns, 1);
    }
}
