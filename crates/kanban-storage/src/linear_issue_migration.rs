//! Idempotent application of migration 53 for recovery re-runs.

use rusqlite::{Connection, OptionalExtension, params};

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
            UNION ALL SELECT 'Duplicate', 'duplicate', 'duplicate', 12
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
    backfill_issue_state_history_from_timeline(conn)?;

    Ok(())
}

pub(crate) fn apply_linear_issue_model_in_span(span: &WriteSpan<'_>) -> rusqlite::Result<()> {
    apply_linear_issue_model(span)
}

fn workflow_id_for_legacy(
    conn: &Connection,
    project_id: i64,
    legacy: &str,
) -> rusqlite::Result<Option<i64>> {
    conn.query_row(
        "SELECT id FROM issue_workflow_states
         WHERE project_id = ?1 AND legacy_state = ?2",
        params![project_id, legacy],
        |row| row.get(0),
    )
    .optional()
}

/// Rebuilds per-ticket state history from legacy timeline transitions.
fn backfill_issue_state_history_from_timeline(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM issue_state_history", [])?;
    let mut tickets = conn.prepare("SELECT id, project_id, created_at FROM tickets ORDER BY id")?;
    let rows = tickets.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    for ticket in rows {
        let (ticket_id, project_id, created_at) = ticket?;
        let mut transitions = conn.prepare(
            "SELECT recorded_at, detail FROM timeline_events
             WHERE kind = 'transition' AND entity_kind = 'ticket' AND entity_id = ?1
             ORDER BY recorded_at ASC, rowid ASC",
        )?;
        let events: Vec<(String, String, String)> = transitions
            .query_map([ticket_id.to_string()], |row| {
                let detail_raw: String = row.get(1)?;
                let detail: serde_json::Value =
                    serde_json::from_str(&detail_raw).unwrap_or(serde_json::Value::Null);
                let from = detail
                    .get("from")
                    .and_then(|value| value.as_str())
                    .unwrap_or("")
                    .to_owned();
                let to = detail
                    .get("to")
                    .and_then(|value| value.as_str())
                    .unwrap_or("")
                    .to_owned();
                Ok((row.get(0)?, from, to))
            })?
            .filter_map(|row| row.ok())
            // Same-state events record a visit, not a transition, so they
            // neither open nor close a history row.
            .filter(|(_, from, to)| !from.is_empty() && !to.is_empty() && from != to)
            .collect::<Vec<_>>();
        if events.is_empty() {
            let workflow_state_id: Option<i64> = conn
                .query_row(
                    "SELECT workflow_state_id FROM tickets WHERE id = ?1",
                    [ticket_id],
                    |row| row.get(0),
                )
                .optional()?
                .flatten();
            if let Some(workflow_state_id) = workflow_state_id {
                conn.execute(
                    "INSERT INTO issue_state_history
                         (ticket_id, workflow_state_id, started_at, ended_at)
                     VALUES (?1, ?2, ?3, NULL)",
                    params![ticket_id, workflow_state_id, created_at],
                )?;
            }
            continue;
        }
        for (index, (recorded_at, from, to)) in events.iter().enumerate() {
            if index == 0
                && let Some(from_id) = workflow_id_for_legacy(conn, project_id, from)?
            {
                conn.execute(
                    "INSERT INTO issue_state_history
                         (ticket_id, workflow_state_id, started_at, ended_at)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![ticket_id, from_id, created_at, recorded_at],
                )?;
            }
            let ended_at = events.get(index + 1).map(|(time, _, _)| time.as_str());
            if let Some(to_id) = workflow_id_for_legacy(conn, project_id, to)? {
                conn.execute(
                    "INSERT INTO issue_state_history
                         (ticket_id, workflow_state_id, started_at, ended_at)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![ticket_id, to_id, recorded_at, ended_at],
                )?;
            }
        }
    }
    collapse_consecutive_duplicate_history(conn)?;
    ensure_current_state_history_row(conn)?;
    backfill_lifecycle_timestamps_from_history(conn)?;
    backfill_legacy_trait_labels(conn)?;
    Ok(())
}

/// Collapses consecutive history rows that repeat one workflow state.
fn collapse_consecutive_duplicate_history(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "DELETE FROM issue_state_history
         WHERE id IN (
             SELECT h2.id
             FROM issue_state_history AS h2
             JOIN issue_state_history AS h1
               ON h1.ticket_id = h2.ticket_id
              AND h1.id = (
                  SELECT MAX(id) FROM issue_state_history
                  WHERE ticket_id = h2.ticket_id AND id < h2.id
              )
             WHERE h1.workflow_state_id = h2.workflow_state_id
         )",
    )
}

/// Guarantees every issue keeps one open history row for its current
/// workflow state, including issues whose only legacy events revisited
/// that state and collapsed away entirely, and issues an earlier build
/// left with their current state's latest row closed: that row reopens
/// rather than gaining a duplicate beside it.
fn ensure_current_state_history_row(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "UPDATE issue_state_history
         SET ended_at = NULL
         WHERE ended_at IS NOT NULL
           AND id IN (
               SELECT h.id
               FROM issue_state_history AS h
               JOIN tickets AS t ON t.id = h.ticket_id
               WHERE t.workflow_state_id = h.workflow_state_id
                 AND h.id = (
                     SELECT MAX(h2.id) FROM issue_state_history AS h2
                     WHERE h2.ticket_id = h.ticket_id
                       AND h2.workflow_state_id = t.workflow_state_id
                 )
                 AND NOT EXISTS (
                     SELECT 1 FROM issue_state_history AS h3
                     WHERE h3.ticket_id = h.ticket_id AND h3.ended_at IS NULL
                 )
           );
         INSERT INTO issue_state_history (ticket_id, workflow_state_id, started_at, ended_at)
         SELECT t.id, t.workflow_state_id, COALESCE(
             (SELECT MAX(COALESCE(h.ended_at, h.started_at))
              FROM issue_state_history AS h
              WHERE h.ticket_id = t.id),
             t.created_at
         ), NULL
         FROM tickets AS t
         WHERE t.workflow_state_id IS NOT NULL
           AND NOT EXISTS (
               SELECT 1 FROM issue_state_history AS h
               WHERE h.ticket_id = t.id
                 AND h.workflow_state_id = t.workflow_state_id
           );",
    )
}

/// Stamps lifecycle timestamps from the rebuilt state history.
///
/// `started_at` keeps the first entry into a started state. The terminal
/// stamps always mirror the current state, the way a save stamps the
/// entry and a reopen clears it: an issue sitting in a completed or
/// canceled state carries the entry into its last such state, and an
/// issue that left carries none. Both directions are rewritten rather
/// than only filled, because an earlier build of the migration stamped
/// the exit of a terminal visit, leaving reopened issues with a stale
/// stamp and re-entered issues with the exit of the earlier visit.
fn backfill_lifecycle_timestamps_from_history(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "UPDATE tickets SET started_at = (
             SELECT h.started_at
             FROM issue_state_history AS h
             JOIN issue_workflow_states AS s ON s.id = h.workflow_state_id
             WHERE h.ticket_id = tickets.id AND s.type = 'started'
             ORDER BY h.started_at ASC, h.id ASC LIMIT 1
         ) WHERE started_at IS NULL;
         UPDATE tickets SET completed_at = (
             SELECT h.started_at
             FROM issue_state_history AS h
             JOIN issue_workflow_states AS s ON s.id = h.workflow_state_id
             WHERE h.ticket_id = tickets.id AND s.type = 'completed'
             ORDER BY h.started_at DESC, h.id DESC LIMIT 1
         ) WHERE EXISTS (
               SELECT 1 FROM issue_workflow_states AS current_state
               WHERE current_state.id = tickets.workflow_state_id
                 AND current_state.type = 'completed'
           );
         UPDATE tickets SET completed_at = NULL
         WHERE NOT EXISTS (
               SELECT 1 FROM issue_workflow_states AS current_state
               WHERE current_state.id = tickets.workflow_state_id
                 AND current_state.type = 'completed'
           );
         UPDATE tickets SET canceled_at = (
             SELECT h.started_at
             FROM issue_state_history AS h
             JOIN issue_workflow_states AS s ON s.id = h.workflow_state_id
             WHERE h.ticket_id = tickets.id AND s.type IN ('canceled', 'duplicate')
             ORDER BY h.started_at DESC, h.id DESC LIMIT 1
         ) WHERE EXISTS (
               SELECT 1 FROM issue_workflow_states AS current_state
               WHERE current_state.id = tickets.workflow_state_id
                 AND current_state.type IN ('canceled', 'duplicate')
           );
         UPDATE tickets SET canceled_at = NULL
         WHERE NOT EXISTS (
               SELECT 1 FROM issue_workflow_states AS current_state
               WHERE current_state.id = tickets.workflow_state_id
                 AND current_state.type IN ('canceled', 'duplicate')
           );",
    )
}

fn backfill_legacy_trait_labels(conn: &Connection) -> rusqlite::Result<()> {
    let mut tickets =
        conn.prepare("SELECT id, project_id, kind, bug_qualification, subtype, mode FROM tickets")?;
    let rows = tickets.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<String>>(5)?,
        ))
    })?;
    for row in rows {
        let (ticket_id, project_id, kind, qualification, subtype, mode) = row?;
        if kind == "bug"
            && let Some(raw) = qualification
            && let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw)
            && let Some(severity) = value.get("severity").and_then(|v| v.as_str())
        {
            // An issue carries at most one severity and one mode, so those
            // groups behave like the single-select Type group.
            ensure_trait_label(conn, project_id, ticket_id, "Severity", severity, true)?;
        }
        if let Some(subtype) = subtype.filter(|value| !value.is_empty()) {
            ensure_trait_label(conn, project_id, ticket_id, "Subtype", &subtype, false)?;
        }
        if let Some(mode) = mode.filter(|value| !value.is_empty()) {
            ensure_trait_label(conn, project_id, ticket_id, "Mode", &mode, true)?;
        }
    }
    Ok(())
}

fn ensure_trait_label(
    conn: &Connection,
    project_id: i64,
    ticket_id: i64,
    group: &str,
    name: &str,
    single_select: bool,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO issue_label_groups (project_id, name, single_select)
         VALUES (?1, ?2, ?3)",
        params![project_id, group, i64::from(single_select)],
    )?;
    conn.execute(
        "UPDATE issue_label_groups SET single_select = ?3
         WHERE project_id = ?1 AND name = ?2",
        params![project_id, group, i64::from(single_select)],
    )?;
    let group_id: i64 = conn.query_row(
        "SELECT id FROM issue_label_groups WHERE project_id = ?1 AND name = ?2",
        params![project_id, group],
        |row| row.get(0),
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO issue_labels (project_id, name, color, group_id)
         VALUES (?1, ?2, '#9CA3AF', ?3)",
        params![project_id, name, group_id],
    )?;
    let label_id: i64 = conn.query_row(
        "SELECT id FROM issue_labels WHERE project_id = ?1 AND name = ?2",
        params![project_id, name],
        |row| row.get(0),
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO issue_ticket_labels (ticket_id, label_id) VALUES (?1, ?2)",
        params![ticket_id, label_id],
    )?;
    Ok(())
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
    add_column(
        conn,
        "tickets",
        "parent_ticket_id",
        "parent_ticket_id INTEGER REFERENCES tickets (id)",
    )?;
    add_column(
        conn,
        "tickets",
        "sub_issue_sort",
        "sub_issue_sort REAL NOT NULL DEFAULT 0",
    )?;
    if tickets_table_has_column(conn, "description")? {
        normalise_legacy_due_dates(conn)?;
        conn.execute_batch(
            "DROP TRIGGER IF EXISTS tickets_bug_shape_insert;
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
        backfill_issue_descriptions(conn)?;
        preserve_legacy_external_blockers(conn)?;
        backfill_issue_state_history_from_timeline(conn)?;
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
                 'superseded', 'duplicate'
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
             parent_ticket_id INTEGER REFERENCES tickets (id),
             sub_issue_sort REAL NOT NULL DEFAULT 0,
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
             parent_ticket_id, sub_issue_sort,
             description, updated_at, version, created_at
         )
         SELECT
             id, project_id, number, kind, priority, state, spec_id, title, slice,
             criteria, actual_behaviour, reporter_evidence, bug_qualification, bug_facts,
             subtype, mode, completion, scheduled_for, due, profile, pinned_version,
             predecessor_id, workflow_state_id, linear_priority, due_date, estimate,
             milestone_id, started_at, completed_at, canceled_at, duplicate_of_ticket_id,
             NULL, 0,
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
         END;",
    )?;
    normalise_legacy_due_dates(conn)?;
    backfill_issue_descriptions(conn)?;
    preserve_legacy_external_blockers(conn)?;
    backfill_issue_state_history_from_timeline(conn)?;
    dedupe_issue_state_history(conn)?;
    backfill_ticket_updated_at_from_timeline(conn)?;
    Ok(())
}

/// Converts legacy `due` timestamps into Linear calendar due dates.
///
/// Legacy values are UTC instants such as `2026-11-15T17:00:00.000Z`;
/// Linear stores a plain `YYYY-MM-DD`. The UTC calendar date is used so
/// the result is deterministic regardless of the migrating machine's
/// time zone, and it matches the dates the legacy data was authored
/// with: the source data was written in Europe/London, which shares
/// UTC's calendar while this data was recorded.
fn normalise_legacy_due_dates(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "UPDATE tickets
         SET due_date = CASE
             WHEN due IS NOT NULL AND due <> '' AND length(due) >= 10 THEN substr(due, 1, 10)
             ELSE due_date
         END
         WHERE due IS NOT NULL AND due <> '' AND due_date = '2099-12-31';",
    )?;
    truncate_datetime_due_dates(conn)
}

/// Truncates datetime-shaped due dates to their calendar date.
///
/// An earlier build copied legacy `due` timestamps into `due_date`
/// verbatim. The old model normalised every stored instant to one
/// exact UTC shape, `YYYY-MM-DDTHH:MM:SS.mmmZ`, so the repair
/// converts only that shape. Any other stored text is a deliberate
/// post-migration edit the issue store's calendar validation accepted
/// verbatim, such as `2027-1-5`, ` 2027-01-05` or a signed year, and
/// it keeps its bytes. The repair also never
/// falls back to the legacy `due` column: once migration has finished,
/// a due date cleared through the issue store is stored as the
/// no-date value while `due` keeps its historical timestamp, and
/// restoring that timestamp would undo a deliberate edit.
fn truncate_datetime_due_dates(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "UPDATE tickets
         SET due_date = substr(due_date, 1, 10)
         WHERE due_date GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z';",
    )
}

fn dedupe_issue_state_history(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM issue_state_history
         WHERE rowid NOT IN (
             SELECT MIN(rowid)
             FROM issue_state_history
             GROUP BY ticket_id, workflow_state_id, started_at, COALESCE(ended_at, '')
         )",
        [],
    )?;
    Ok(())
}

fn backfill_ticket_updated_at_from_timeline(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE tickets
         SET updated_at = COALESCE(
             (SELECT MAX(recorded_at) FROM timeline_events
              WHERE entity_kind = 'ticket' AND entity_id = CAST(tickets.id AS TEXT)),
             updated_at
         )",
        [],
    )?;
    Ok(())
}

struct SpecVersionSections {
    name: String,
    short_description: String,
    problem_statement: String,
    solution: String,
    user_stories: String,
    implementation_decisions: String,
    testing_decisions: String,
    out_of_scope: String,
    further_notes: String,
}

fn load_approved_spec_sections(
    conn: &Connection,
    spec_id: i64,
    pinned_version: Option<i64>,
) -> rusqlite::Result<Option<SpecVersionSections>> {
    let version_number: Option<i64> = if let Some(number) = pinned_version {
        Some(number)
    } else {
        conn.query_row(
            "SELECT MAX(number) FROM spec_versions
             WHERE spec_id = ?1 AND state = 'approved'",
            [spec_id],
            |row| row.get(0),
        )
        .optional()?
        .flatten()
    };
    let Some(number) = version_number else {
        return Ok(None);
    };
    match conn.query_row(
        "SELECT name, short_description, problem_statement, solution, user_stories,
                implementation_decisions, testing_decisions, out_of_scope, further_notes
         FROM spec_versions
         WHERE spec_id = ?1 AND number = ?2",
        params![spec_id, number],
        |row| {
            Ok(SpecVersionSections {
                name: row.get(0)?,
                short_description: row.get(1)?,
                problem_statement: row.get(2)?,
                solution: row.get(3)?,
                user_stories: row.get(4)?,
                implementation_decisions: row.get(5)?,
                testing_decisions: row.get(6)?,
                out_of_scope: row.get(7)?,
                further_notes: row.get(8)?,
            })
        },
    ) {
        Ok(sections) => Ok(Some(sections)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(error) => Err(error),
    }
}

fn push_section(document: &mut String, heading: &str, body: &str) {
    document.push_str(&format!("\n### {heading}\n\n{body}\n"));
}

fn render_spec_sections(sections: &SpecVersionSections) -> String {
    let mut document = format!("\n## Spec — {}\n", sections.name);
    push_section(
        &mut document,
        "Short description",
        &sections.short_description,
    );
    push_section(
        &mut document,
        "Problem statement",
        &sections.problem_statement,
    );
    push_section(&mut document, "Solution", &sections.solution);
    push_section(&mut document, "User stories", &sections.user_stories);
    push_section(
        &mut document,
        "Implementation decisions",
        &sections.implementation_decisions,
    );
    push_section(
        &mut document,
        "Testing decisions",
        &sections.testing_decisions,
    );
    push_section(&mut document, "Out of scope", &sections.out_of_scope);
    push_section(&mut document, "Further notes", &sections.further_notes);
    document
}

struct TicketDescriptionSource {
    kind: String,
    slice: Option<String>,
    title: Option<String>,
    spec_id: Option<i64>,
    pinned_version: Option<i64>,
    actual_behaviour: Option<String>,
    reporter_evidence: Option<String>,
}

fn build_issue_description(
    conn: &Connection,
    source: &TicketDescriptionSource,
) -> rusqlite::Result<String> {
    let mut document = String::new();
    match source.kind.as_str() {
        "implementation" => {
            if let Some(slice) = source.slice.as_ref().filter(|value| !value.is_empty()) {
                document.push_str(&format!("\n## Slice\n\n{slice}\n"));
            }
            if let Some(spec_id) = source.spec_id
                && let Some(sections) =
                    load_approved_spec_sections(conn, spec_id, source.pinned_version)?
            {
                document.push_str(&render_spec_sections(&sections));
            }
        }
        "bug" => {
            if let Some(title) = source.title.as_ref().filter(|value| !value.is_empty()) {
                document.push_str(&format!("\n## Title\n\n{title}\n"));
            }
            if let Some(actual) = source
                .actual_behaviour
                .as_ref()
                .filter(|value| !value.is_empty())
            {
                push_section(&mut document, "Actual behaviour", actual);
            }
            if let Some(evidence) = source
                .reporter_evidence
                .as_ref()
                .filter(|value| !value.is_empty())
            {
                push_section(&mut document, "Reporter evidence", evidence);
            }
        }
        "task" => {
            if let Some(title) = source.title.as_ref().filter(|value| !value.is_empty()) {
                document.push_str(&format!("\n## Title\n\n{title}\n"));
            }
        }
        _ => {}
    }
    Ok(document.trim_start().to_owned())
}

/// Fills empty issue descriptions from legacy ticket and spec content.
pub(crate) fn backfill_issue_descriptions(conn: &Connection) -> rusqlite::Result<()> {
    if !tickets_table_has_column(conn, "description")? {
        return Ok(());
    }
    let mut statement = conn.prepare(
        "SELECT id, kind, slice, title, spec_id, pinned_version, actual_behaviour, reporter_evidence
         FROM tickets
         WHERE description = ''",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<i64>>(4)?,
            row.get::<_, Option<i64>>(5)?,
            row.get::<_, Option<String>>(6)?,
            row.get::<_, Option<String>>(7)?,
        ))
    })?;
    for row in rows {
        let (
            ticket_id,
            kind,
            slice,
            title,
            spec_id,
            pinned_version,
            actual_behaviour,
            reporter_evidence,
        ) = row?;
        let is_task = kind == "task";
        let mut description = build_issue_description(
            conn,
            &TicketDescriptionSource {
                kind,
                slice,
                title,
                spec_id,
                pinned_version,
                actual_behaviour,
                reporter_evidence,
            },
        )?;
        if is_task && let Some(checklist) = task_checklist_markdown(conn, ticket_id)? {
            description = append_checklist(&description, &checklist);
        }
        if description.is_empty() {
            continue;
        }
        conn.execute(
            "UPDATE tickets SET description = ?2 WHERE id = ?1",
            params![ticket_id, description],
        )?;
    }
    Ok(())
}

pub(crate) fn repair_task_checklists(conn: &Connection) -> rusqlite::Result<()> {
    if !tickets_table_has_column(conn, "description")? {
        return Ok(());
    }
    let mut statement =
        conn.prepare("SELECT id, description FROM tickets WHERE kind = 'task' ORDER BY id")?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    for (ticket_id, description) in rows {
        let Some(repaired) = with_task_checklist(conn, ticket_id, "task", description)? else {
            continue;
        };
        let current: String = conn.query_row(
            "SELECT description FROM tickets WHERE id = ?1",
            [ticket_id],
            |row| row.get(0),
        )?;
        if repaired == current {
            continue;
        }
        conn.execute(
            "UPDATE tickets SET description = ?2 WHERE id = ?1",
            params![ticket_id, repaired],
        )?;
    }
    Ok(())
}

fn with_task_checklist(
    conn: &Connection,
    ticket_id: i64,
    kind: &str,
    description: String,
) -> rusqlite::Result<Option<String>> {
    if kind != "task" {
        return Ok(Some(description));
    }
    let Some(checklist) = task_checklist_markdown(conn, ticket_id)? else {
        return Ok(Some(description));
    };
    Ok(Some(append_checklist(&description, &checklist)))
}

fn task_checklist_markdown(conn: &Connection, ticket_id: i64) -> rusqlite::Result<Option<String>> {
    let completion: String = conn.query_row(
        "SELECT completion FROM tickets WHERE id = ?1",
        [ticket_id],
        |row| row.get(0),
    )?;
    let outcomes: Vec<String> = serde_json::from_str(&completion)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    if outcomes.is_empty() {
        return Ok(None);
    }
    let mut met = std::collections::BTreeSet::new();
    let mut bindings = conn.prepare(
        "SELECT criterion_index FROM criterion_bindings
         WHERE ticket_id = ?1 AND kind = 'task' AND satisfied = 1 AND void = 0",
    )?;
    let indexes = bindings.query_map([ticket_id], |row| row.get::<_, i64>(0))?;
    for index in indexes {
        met.insert(index?);
    }
    let mut checklist = String::new();
    for (index, outcome) in outcomes.iter().enumerate() {
        let mark = if met.contains(&(index as i64)) {
            "x"
        } else {
            " "
        };
        checklist.push_str(&format!("- [{mark}] {outcome}\n"));
    }
    Ok(Some(checklist.trim_end().to_owned()))
}

fn append_checklist(description: &str, checklist: &str) -> String {
    if description.contains(checklist) || checklist_already_present(description, checklist) {
        return description.to_owned();
    }
    let mut document = description.to_owned();
    if document.is_empty() {
        document.push_str(checklist);
    } else {
        if !document.ends_with('\n') {
            document.push('\n');
        }
        document.push('\n');
        document.push_str(checklist);
    }
    if !document.ends_with('\n') {
        document.push('\n');
    }
    document
}
fn checklist_already_present(description: &str, checklist: &str) -> bool {
    let criteria: Vec<&str> = checklist.lines().filter_map(checkbox_criterion).collect();
    if criteria.is_empty() {
        return false;
    }
    criteria.iter().any(|criterion| {
        description
            .lines()
            .any(|line| checkbox_criterion(line) == Some(*criterion))
    })
}

fn checkbox_criterion(line: &str) -> Option<&str> {
    let rest = line.trim().strip_prefix("- [")?;
    let mut chars = rest.chars();
    chars.next()?;
    if chars.next() != Some(']') {
        return None;
    }
    let criterion = chars.as_str().trim_start();
    (!criterion.is_empty()).then_some(criterion)
}

pub(crate) fn apply_task_checklist_repair_in_span(span: &WriteSpan<'_>) -> rusqlite::Result<()> {
    repair_task_checklists(span)
}

/// Appends legacy external blocker prose to issue descriptions.
fn preserve_legacy_external_blockers(conn: &Connection) -> rusqlite::Result<()> {
    if !tickets_table_has_column(conn, "description")? {
        return Ok(());
    }
    let mut tickets =
        conn.prepare("SELECT DISTINCT ticket_id FROM ticket_blockers ORDER BY ticket_id")?;
    let ticket_ids = tickets
        .query_map([], |row| row.get::<_, i64>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    for ticket_id in ticket_ids {
        let mut blockers = conn
            .prepare("SELECT description FROM ticket_blockers WHERE ticket_id = ?1 ORDER BY id")?;
        let descriptions: Vec<String> = blockers
            .query_map([ticket_id], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        if descriptions.is_empty() {
            continue;
        }
        let current: String = conn.query_row(
            "SELECT description FROM tickets WHERE id = ?1",
            [ticket_id],
            |row| row.get(0),
        )?;
        let marker = "\n## External blockers\n";
        if current.contains("## External blockers") {
            continue;
        }
        let mut document = current;
        if !document.is_empty() && !document.ends_with('\n') {
            document.push('\n');
        }
        document.push_str(marker);
        for text in descriptions {
            document.push_str(&format!("\n- {text}\n"));
        }
        conn.execute(
            "UPDATE tickets SET description = ?2 WHERE id = ?1",
            params![ticket_id, document],
        )?;
    }
    Ok(())
}

/// Ensures Duplicate workflow maps to its own legacy ticket state and the
/// `tickets.state` CHECK accepts `duplicate`.
pub(crate) fn apply_duplicate_ticket_state(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE issue_workflow_states SET legacy_state = 'duplicate' WHERE name = 'Duplicate'",
        [],
    )?;
    conn.execute_batch(
        "INSERT OR IGNORE INTO issue_workflow_states (project_id, name, type, legacy_state, position)
         SELECT p.id, 'Duplicate', 'duplicate', 'duplicate', 12 FROM projects AS p",
    )?;
    let ddl: String = conn.query_row(
        "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'tickets'",
        [],
        |row| row.get(0),
    )?;
    if ddl.contains("'duplicate'") {
        return Ok(());
    }
    conn.execute_batch(include_str!(
        "../migrations/0055_duplicate_ticket_state.sql"
    ))?;
    Ok(())
}

pub(crate) fn apply_comment_thread_columns(conn: &Connection) -> rusqlite::Result<()> {
    add_column(
        conn,
        "comments",
        "parent_comment_id",
        "parent_comment_id INTEGER REFERENCES comments (id)",
    )?;
    add_column(conn, "comments", "resolved_at", "resolved_at TEXT")?;
    Ok(())
}

pub(crate) fn apply_comment_thread_columns_in_span(span: &WriteSpan<'_>) -> rusqlite::Result<()> {
    apply_comment_thread_columns(span)
}

/// Adds comment authorship and backfills from timeline JSON where present.
pub(crate) fn apply_comment_author_column(conn: &Connection) -> rusqlite::Result<()> {
    if table_has_column(conn, "comments", "author")? {
        return Ok(());
    }
    conn.execute_batch(
        "ALTER TABLE comments ADD COLUMN author TEXT NOT NULL DEFAULT '';
         UPDATE comments
         SET author = COALESCE(
             (SELECT json_extract(detail, '$.author')
              FROM timeline_events
              WHERE kind = 'comment'
                AND json_extract(detail, '$.comment_id') = comments.id
              ORDER BY recorded_at DESC
              LIMIT 1),
             ''
         );",
    )
}

pub(crate) fn apply_duplicate_ticket_state_in_span(span: &WriteSpan<'_>) -> rusqlite::Result<()> {
    apply_duplicate_ticket_state(span)
}

pub(crate) fn apply_comment_author_column_in_span(span: &WriteSpan<'_>) -> rusqlite::Result<()> {
    apply_comment_author_column(span)
}

pub(crate) fn apply_project_auto_close_setting(conn: &Connection) -> rusqlite::Result<()> {
    add_column(
        conn,
        "projects",
        "auto_close_sub_issues",
        "auto_close_sub_issues INTEGER NOT NULL DEFAULT 0",
    )
}

pub(crate) fn apply_project_auto_close_setting_in_span(
    span: &WriteSpan<'_>,
) -> rusqlite::Result<()> {
    apply_project_auto_close_setting(span)
}

pub(crate) fn apply_issue_assignee_column(conn: &Connection) -> rusqlite::Result<()> {
    add_column(
        conn,
        "tickets",
        "assignee",
        "assignee TEXT NOT NULL DEFAULT ''",
    )
}

pub(crate) fn apply_issue_assignee_column_in_span(span: &WriteSpan<'_>) -> rusqlite::Result<()> {
    apply_issue_assignee_column(span)
}

/// Repairs the linear issue migration on databases that applied an
/// earlier build of migrations 53 through 59.
///
/// Those builds copied legacy due timestamps in verbatim, left the
/// terminal stamps empty, dropped the history row of an issue that only
/// revisited its current state, and created the Severity and Mode label
/// groups multi-select. The repair reuses the migration's own rules, so
/// a database this build already migrated is left unchanged and a
/// repeated run changes nothing. Due dates are only truncated to their
/// calendar date: the legacy `due` column is not consulted, because a
/// due date cleared after migration is stored as the no-date value.
pub(crate) fn apply_linear_issue_fidelity(conn: &Connection) -> rusqlite::Result<()> {
    truncate_datetime_due_dates(conn)?;
    ensure_current_state_history_row(conn)?;
    backfill_lifecycle_timestamps_from_history(conn)?;
    conn.execute_batch(
        "UPDATE issue_label_groups
         SET single_select = 1
         WHERE name IN ('Severity', 'Mode');",
    )
}

pub(crate) fn apply_linear_issue_fidelity_in_span(span: &WriteSpan<'_>) -> rusqlite::Result<()> {
    apply_linear_issue_fidelity(span)
}

pub(crate) fn apply_linear_issue_relaxation_in_span(span: &WriteSpan<'_>) -> rusqlite::Result<()> {
    apply_linear_issue_relaxation(span)
}

/// Migration 0061: normalise stored issue dates.
///
/// Earlier builds of this branch accepted due dates and milestone
/// target dates the strict calendar validator now refuses, such as
/// `2026-1-5` or a date with surrounding whitespace. Every stored
/// date that parses as a calendar date is rewritten in its
/// zero-padded `YYYY-MM-DD` spelling; anything unparseable keeps its
/// bytes and is reported nowhere. The due-date column's old in-band
/// no-date marker — the exact date `2099-12-31` every earlier build
/// wrote for "no due date" and read back as none — becomes the empty
/// none value first, so it can never swallow a real far-future
/// deadline again. Milestone target dates never carried the marker,
/// so theirs are only padded.
pub(crate) fn apply_canonical_comment_addresses_in_span(
    span: &WriteSpan<'_>,
) -> rusqlite::Result<()> {
    span.execute_batch(
        "CREATE INDEX IF NOT EXISTS comments_by_target
         ON comments (entity_kind, entity_id, id)",
    )?;
    let mut statement =
        span.prepare("SELECT id, entity_id FROM comments WHERE entity_kind = 'ticket'")?;
    let rows: Vec<(i64, String)> = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    for (row_id, stored) in rows {
        if let Some(canonical) = canonical_ticket_address(span, &stored)?
            && canonical != stored
        {
            span.execute(
                "UPDATE comments SET entity_id = ?2 WHERE id = ?1",
                params![row_id, canonical],
            )?;
        }
    }
    ensure_comment_target_key(span)
}

fn ensure_comment_target_key(span: &WriteSpan<'_>) -> rusqlite::Result<()> {
    let present: i64 = span.query_row(
        "SELECT COUNT(*) FROM sqlite_master
         WHERE type = 'table' AND name = 'comments' AND sql LIKE '%target_key%'",
        [],
        |row| row.get(0),
    )?;
    if present == 0 {
        span.execute_batch(include_str!("../migrations/comment_target_key.sql"))?;
    } else {
        span.execute_batch(
            "CREATE INDEX IF NOT EXISTS comments_by_canonical_target
             ON comments (entity_kind, target_key, id)",
        )?;
    }
    Ok(())
}

fn canonical_ticket_address(conn: &Connection, stored: &str) -> rusqlite::Result<Option<String>> {
    if let Ok(id) = stored.parse::<i64>() {
        let found: Option<i64> = conn
            .query_row("SELECT id FROM tickets WHERE id = ?1", [id], |row| {
                row.get(0)
            })
            .optional()?;
        return Ok(found.map(|ticket| ticket.to_string()));
    }
    let Some(key) = kanban_domain::IssueKey::parse(stored) else {
        return Ok(None);
    };
    let found: Option<i64> = conn
        .query_row(
            "SELECT t.id FROM tickets AS t
             JOIN projects AS p ON p.id = t.project_id
             WHERE UPPER(p.code) = UPPER(?1) AND t.number = ?2",
            params![key.project_code().as_str(), key.number().value() as i64],
            |row| row.get(0),
        )
        .optional()?;
    Ok(found.map(|ticket| ticket.to_string()))
}

pub(crate) fn apply_loose_issue_date_normalisation(span: &WriteSpan<'_>) -> rusqlite::Result<()> {
    span.execute_batch("UPDATE tickets SET due_date = '' WHERE due_date = '2099-12-31';")?;
    for table_and_column in [("tickets", "due_date"), ("issue_milestones", "target_date")] {
        let (table, column) = table_and_column;
        let rows: Vec<(i64, String)> = {
            let mut statement = span.prepare(&format!(
                "SELECT rowid, {column} FROM {table}
                 WHERE {column} IS NOT NULL AND {column} <> ''"
            ))?;
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<Result<_, _>>()?
        };
        for (rowid, raw) in rows {
            if let Some(padded) = padded_calendar_date(&raw)
                && padded != raw
            {
                span.execute(
                    &format!("UPDATE {table} SET {column} = ?2 WHERE rowid = ?1"),
                    rusqlite::params![rowid, padded],
                )?;
            }
        }
    }
    Ok(())
}

/// The zero-padded spelling of a date that parses as a calendar
/// date once surrounding whitespace is trimmed, or `None` for
/// anything the calendar validator would refuse.
fn padded_calendar_date(raw: &str) -> Option<String> {
    chrono::NaiveDate::parse_from_str(raw.trim(), "%Y-%m-%d")
        .ok()
        .map(|date| date.format("%Y-%m-%d").to_string())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::num::NonZeroU32;
    use std::sync::Arc;

    use kanban_app::catalog::exposed_operations;
    use kanban_app::events::NoopEventSink;
    use kanban_app::{
        CommentIssueTargets, CommentStore, Core, DependencyStore, IssueStore,
        IssueStoreCommentTargets, OperatorSession, ProjectStore, TicketStore,
    };
    use kanban_domain::{
        CommentId, DependencyState, ProjectRegistration, ReadinessBlocker, ReadinessInputs,
        TicketDependencyGraph, TicketId, compute_readiness,
    };
    use kanban_dto::{
        ErrorCode, IssueGetQuery, IssueRecord, TimelineEntityKind, TimelineEntityRef,
        TimelineEventKind,
    };
    use serde_json::json;

    use super::*;
    use crate::comments::SqliteCommentStore;
    use crate::db::Database;
    use crate::dependencies::SqliteDependencyStore;
    use crate::evidence::SqliteEvidenceStore;
    use crate::idempotency::{RetentionPolicy, SqliteIdempotencyStore};
    use crate::issue_store::SqliteIssueStore as IssueStoreImpl;
    use crate::migrations::{self, AllowAllMigrations};
    use crate::plan::SqlitePlanStore;
    use crate::projects::SqliteProjectStore;
    use crate::spec::SqliteSpecStore;
    use crate::tickets::SqliteTicketStore;

    const PROBLEM: &str = "Approved PRD problem line for migration";
    const COMMENT_AUTHOR: &str = "ada";
    const COMMENT_CREATED: &str = "2020-06-01T10:00:00.000000Z";
    const COMMENT_EDITED: &str = "2020-06-02T11:00:00.000000Z";

    fn seed_project(database: &Database) {
        let projects = SqliteProjectStore::new(database);
        let registration = ProjectRegistration::new(
            "CORE",
            "Control plane",
            "/repositories/kanban",
            "/workspaces/kanban.seed",
            "main",
            "kanban.seed",
            Some("kanban-main"),
            None,
        )
        .unwrap();
        projects
            .create(&registration, &|id| {
                kanban_app::TimelineEnvelope::project(
                    id.value(),
                    TimelineEventKind::Transition,
                    Some(TimelineEntityRef {
                        kind: TimelineEntityKind::Project,
                        id: id.value().to_string(),
                    }),
                    json!({ "action": "registered" }),
                )
            })
            .unwrap();
    }

    fn seed_schema_52_fixture(conn: &Connection) {
        conn.execute(
            "INSERT INTO specs (id, project_id, number, execution, plan_id, version)
             VALUES (1, 1, 1, 'planned', NULL, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO spec_versions (
                 spec_id, number, state, name, short_description, problem_statement,
                 solution, user_stories, implementation_decisions, testing_decisions,
                 out_of_scope, further_notes
             ) VALUES (
                 1, 1, 'approved', 'Login PRD', 'Short scope', ?1,
                 'Build login', 'As a user', 'Use OAuth', 'Integration tests',
                 'Nothing', 'None'
             )",
            [PROBLEM],
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO tickets (
                 id, project_id, number, kind, priority, state, spec_id, title, slice,
                 criteria, subtype, mode, completion, version, created_at
             ) VALUES (
                 1, 1, 1, 'implementation', 'normal', 'done', 1, NULL, 'Login flow',
                 '[{\"outcome\":\"User logs in\",\"stories\":[{\"spec\":1,\"story\":1}]}]', NULL, NULL, '[]', 1,
                 '2019-01-01T00:00:00.000000Z'
             );
             INSERT INTO tickets (
                 id, project_id, number, kind, priority, state, spec_id, title, slice,
                 criteria, subtype, mode, completion, version, created_at
             ) VALUES (
                 2, 1, 2, 'task', 'normal', 'draft', NULL, 'Wait on login', NULL,
                 '[]', 'operational', 'human', '[\"Shipped\"]', 1,
                 '2019-01-02T00:00:00.000000Z'
             );
             INSERT INTO ticket_dependencies (from_ticket, to_ticket) VALUES (1, 2);
             INSERT INTO ticket_blockers (ticket_id, description)
             VALUES (2, 'Vendor API quota holds the rollout');
             INSERT INTO comments (id, project_id, entity_kind, entity_id, version, created_at)
             VALUES (1, '1', 'ticket', '2', 2, '2020-06-01T10:00:00.000000Z');
             INSERT INTO comment_revisions (comment_id, revision, text, recorded_at)
             VALUES (1, 1, 'First note', '2020-06-01T10:00:00.000000Z');
             INSERT INTO comment_revisions (comment_id, revision, text, recorded_at)
             VALUES (1, 2, 'Edited note', '2020-06-02T11:00:00.000000Z');",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO timeline_events (kind, detail, recorded_at, project_id, entity_kind, entity_id)
             VALUES (
                 'comment',
                 '{\"comment_id\":1,\"text\":\"First note\",\"revision\":1,\"recorded_at\":\"2020-06-01T10:00:00.000000Z\",\"author\":\"ada\"}',
                 '2020-06-01T10:00:00.000000Z', '1', 'ticket', '2'
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO timeline_events (kind, detail, recorded_at, project_id, entity_kind, entity_id)
             VALUES ('comment', ?1, ?2, '1', 'ticket', '2')",
            [format!(
                "{{\"comment_id\":1,\"text\":\"Edited note\",\"revision\":2,\"recorded_at\":\"{COMMENT_EDITED}\",\"author\":\"{COMMENT_AUTHOR}\"}}"
            ), COMMENT_EDITED.to_owned()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO timeline_events (kind, detail, recorded_at, project_id, entity_kind, entity_id)
             VALUES (
                 'transition',
                 '{\"from\":\"draft\",\"to\":\"active\"}',
                 '2019-01-01T12:00:00.000000Z', '1', 'ticket', '1'
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO timeline_events (kind, detail, recorded_at, project_id, entity_kind, entity_id)
             VALUES (
                 'transition',
                 '{\"from\":\"active\",\"to\":\"done\"}',
                 '2019-01-02T12:00:00.000000Z', '1', 'ticket', '1'
             )",
            [],
        )
        .unwrap();
    }
    fn seed_legacy_task(
        conn: &Connection,
        id: i64,
        state: &str,
        due: Option<&str>,
        created_at: &str,
    ) {
        conn.execute(
            "INSERT INTO tickets (
                 id, project_id, number, kind, priority, state, title, criteria,
                 subtype, mode, completion, due, version, created_at
             ) VALUES (
                 ?1, 1, ?1, 'task', 'normal', ?2, ?3, '[]',
                 'operational', 'human', '[\"Shipped\"]', ?4, 1, ?5
             )",
            params![id, state, format!("Legacy task {id}"), due, created_at],
        )
        .unwrap();
    }

    fn seed_legacy_bug(
        conn: &Connection,
        id: i64,
        state: &str,
        severity: Option<&str>,
        created_at: &str,
    ) {
        conn.execute(
            "INSERT INTO tickets (
                 id, project_id, number, kind, priority, state, title, criteria,
                 actual_behaviour, reporter_evidence, bug_qualification, bug_facts,
                 completion, version, created_at
             ) VALUES (
                 ?1, 1, ?1, 'bug', 'normal', ?2, ?3, '[]',
                 'Observed behaviour', 'Board screenshot',
                 ?4, '{\"external_references\":[]}',
                 '[]', 1, ?5
             )",
            params![
                id,
                state,
                format!("Legacy bug {id}"),
                severity.map(|value| format!("{{\"severity\":\"{value}\"}}")),
                created_at
            ],
        )
        .unwrap();
    }

    fn seed_legacy_transition(
        conn: &Connection,
        ticket_id: i64,
        from: &str,
        to: &str,
        recorded_at: &str,
    ) {
        conn.execute(
            "INSERT INTO timeline_events (
                 kind, detail, recorded_at, project_id, entity_kind, entity_id
             ) VALUES (
                 'transition', ?1, ?2, '1', 'ticket', ?3
             )",
            params![
                format!("{{\"from\":\"{from}\",\"to\":\"{to}\"}}"),
                recorded_at,
                ticket_id.to_string()
            ],
        )
        .unwrap();
    }

    fn migrated_due_date(conn: &Connection, ticket_id: i64) -> String {
        conn.query_row(
            "SELECT due_date FROM tickets WHERE id = ?1",
            [ticket_id],
            |row| row.get(0),
        )
        .unwrap()
    }

    fn migrated_lifecycle_timestamps(
        conn: &Connection,
        ticket_id: i64,
    ) -> (Option<String>, Option<String>, Option<String>) {
        conn.query_row(
            "SELECT started_at, completed_at, canceled_at FROM tickets WHERE id = ?1",
            [ticket_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap()
    }

    fn migrated_history(
        conn: &Connection,
        ticket_id: i64,
    ) -> Vec<(String, String, Option<String>)> {
        let mut statement = conn
            .prepare(
                "SELECT s.name, h.started_at, h.ended_at
                 FROM issue_state_history AS h
                 JOIN issue_workflow_states AS s ON s.id = h.workflow_state_id
                 WHERE h.ticket_id = ?1
                 ORDER BY h.started_at ASC, h.id ASC",
            )
            .unwrap();
        statement
            .query_map([ticket_id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    /// A database that applied an earlier build of the linear issue
    /// migrations: due dates carry legacy timestamps, terminal stamps
    /// record the exit of a terminal visit instead of its entry, the
    /// history row for an issue that only revisited its current state
    /// collapsed away, and the Severity and Mode groups were created
    /// multi-select.
    fn earlier_build_database() -> (tempfile::TempDir, Database) {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("kanban.sqlite");
        let mut database = Database::open(&path).unwrap();
        migrations::apply_through(&database.connection(), 52).unwrap();
        seed_project(&database);
        {
            let conn = database.connection();
            seed_legacy_task(
                &conn,
                1,
                "active",
                Some("2026-11-15T17:00:00.000Z"),
                "2026-11-03T21:38:37.985Z",
            );
            seed_legacy_transition(&conn, 1, "draft", "active", "2026-11-03T21:39:55.127Z");

            seed_legacy_task(&conn, 2, "done", None, "2026-11-03T21:38:37.981Z");
            seed_legacy_transition(&conn, 2, "draft", "ready", "2026-11-03T21:38:46.601Z");
            seed_legacy_transition(&conn, 2, "ready", "active", "2026-11-03T21:38:50.957Z");
            seed_legacy_transition(&conn, 2, "active", "landing", "2026-11-03T21:39:00.402Z");
            seed_legacy_transition(&conn, 2, "landing", "done", "2026-11-03T21:39:00.405Z");

            seed_legacy_bug(&conn, 3, "cancelled", None, "2026-11-03T21:38:37.996Z");
            seed_legacy_transition(&conn, 3, "draft", "cancelled", "2026-11-03T21:39:55.119Z");

            seed_legacy_bug(
                &conn,
                4,
                "draft",
                Some("critical"),
                "2026-11-03T21:38:37.997Z",
            );
            seed_legacy_transition(&conn, 4, "draft", "draft", "2026-11-03T21:40:21.545Z");

            seed_legacy_task(&conn, 5, "active", None, "2026-11-03T21:38:37.985Z");
            conn.execute(
                "UPDATE tickets SET mode = 'agent', subtype = 'investigative' WHERE id = 5",
                [],
            )
            .unwrap();
            seed_legacy_transition(&conn, 5, "draft", "active", "2026-11-03T21:39:55.127Z");

            seed_legacy_task(&conn, 6, "active", None, "2026-11-03T21:45:00.000Z");
            seed_legacy_transition(&conn, 6, "draft", "done", "2026-11-03T21:45:10.000Z");
            seed_legacy_transition(&conn, 6, "done", "active", "2026-11-03T21:45:20.000Z");

            seed_legacy_task(&conn, 7, "done", None, "2026-11-03T21:46:00.000Z");
            seed_legacy_transition(&conn, 7, "draft", "done", "2026-11-03T21:46:10.000Z");
            seed_legacy_transition(&conn, 7, "done", "active", "2026-11-03T21:46:20.000Z");
            seed_legacy_transition(&conn, 7, "active", "done", "2026-11-03T21:46:30.000Z");

            seed_legacy_bug(&conn, 8, "active", None, "2026-11-03T21:47:00.000Z");
            seed_legacy_transition(&conn, 8, "draft", "cancelled", "2026-11-03T21:47:10.000Z");
            seed_legacy_transition(&conn, 8, "cancelled", "active", "2026-11-03T21:47:20.000Z");

            seed_legacy_bug(&conn, 9, "cancelled", None, "2026-11-03T21:48:00.000Z");
            seed_legacy_transition(&conn, 9, "draft", "cancelled", "2026-11-03T21:48:10.000Z");
            seed_legacy_transition(&conn, 9, "cancelled", "active", "2026-11-03T21:48:20.000Z");
            seed_legacy_transition(&conn, 9, "active", "cancelled", "2026-11-03T21:48:30.000Z");

            seed_legacy_task(&conn, 10, "parked", None, "2026-11-03T21:38:37.988Z");
            seed_legacy_transition(&conn, 10, "draft", "parked", "2026-11-03T21:38:46.608Z");
            seed_legacy_transition(&conn, 10, "parked", "parked", "2026-11-03T21:40:21.542Z");
        }
        database.migrate(&AllowAllMigrations).unwrap();

        database
            .connection()
            .execute_batch(
                "UPDATE tickets SET due_date = '2026-11-15T17:00:00.000Z' WHERE id = 1;
                 UPDATE tickets SET completed_at = (
                     SELECT h.ended_at
                     FROM issue_state_history AS h
                     JOIN issue_workflow_states AS s ON s.id = h.workflow_state_id
                     WHERE h.ticket_id = tickets.id
                       AND s.type = 'completed'
                       AND h.ended_at IS NOT NULL
                     ORDER BY h.ended_at DESC LIMIT 1
                 );
                 UPDATE tickets SET canceled_at = (
                     SELECT h.ended_at
                     FROM issue_state_history AS h
                     JOIN issue_workflow_states AS s ON s.id = h.workflow_state_id
                     WHERE h.ticket_id = tickets.id
                       AND s.type IN ('canceled', 'duplicate')
                       AND h.ended_at IS NOT NULL
                     ORDER BY h.ended_at DESC LIMIT 1
                 );
                 DELETE FROM issue_state_history WHERE ticket_id = 4;
                 UPDATE issue_state_history SET ended_at = '2026-11-03T21:40:21.542Z'
                 WHERE ticket_id = 10 AND ended_at IS NULL;
                 UPDATE issue_label_groups SET single_select = 0
                 WHERE name IN ('Severity', 'Mode');
                 DELETE FROM schema_migrations WHERE version IN (60, 61, 62, 63, 64);",
            )
            .unwrap();
        (dir, database)
    }

    #[derive(Debug, PartialEq, Eq)]
    struct MigratedComment {
        text: String,
        author: String,
        created_at: String,
        edited_at: Option<String>,
        edited: bool,
    }

    fn load_migrated_comment(conn: &Connection, comment_id: i64) -> MigratedComment {
        let (version, created_at): (i64, String) = conn
            .query_row(
                "SELECT version, created_at FROM comments WHERE id = ?1",
                [comment_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let (text, edited_at): (String, String) = conn
            .query_row(
                "SELECT text, recorded_at FROM comment_revisions
                 WHERE comment_id = ?1 ORDER BY revision DESC LIMIT 1",
                [comment_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let detail: String = conn
            .query_row(
                "SELECT detail FROM timeline_events
                 WHERE kind = 'comment' AND detail LIKE '%\"comment_id\":1%'
                 ORDER BY recorded_at DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let author = serde_json::from_str::<serde_json::Value>(&detail).unwrap()["author"]
            .as_str()
            .unwrap()
            .to_owned();
        MigratedComment {
            text,
            author,
            created_at,
            edited_at: (version > 1).then_some(edited_at),
            edited: version > 1,
        }
    }

    fn register_operator_stack(
        core: &mut Core,
        database: &Database,
        attachments: &std::path::Path,
    ) {
        let projects = Arc::new(SqliteProjectStore::new(database));
        let tickets = Arc::new(SqliteTicketStore::new(database));
        let specs = Arc::new(SqliteSpecStore::new(database));
        let plans = Arc::new(SqlitePlanStore::new(database));
        let issues: Arc<dyn IssueStore> = Arc::new(IssueStoreImpl::new(database));
        let comments: Arc<dyn CommentStore> = Arc::new(SqliteCommentStore::new(database));
        core.register_plans(plans.clone(), projects.clone(), specs.clone())
            .unwrap();
        core.register_specs(specs.clone(), projects.clone(), plans)
            .unwrap();
        core.register_tickets(
            tickets.clone(),
            projects.clone(),
            specs.clone(),
            Arc::new(SqliteEvidenceStore::new(
                database,
                attachments.to_path_buf(),
            )),
        )
        .unwrap();
        core.register_issue_operator(
            tickets,
            projects.clone(),
            specs,
            issues.clone(),
            comments.clone(),
        )
        .unwrap();
        let comment_targets: Arc<dyn CommentIssueTargets> =
            Arc::new(IssueStoreCommentTargets::new(issues));
        core.register_comments(comments, projects, comment_targets)
            .unwrap();
    }

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

    #[test]
    fn migration_52_to_54_preserves_spec_description_comments_and_relations() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("kanban.sqlite");
        let mut database = Database::open(&path).unwrap();
        migrations::apply_through(&database.connection(), 52).unwrap();
        seed_project(&database);
        seed_schema_52_fixture(&database.connection());
        database.migrate(&AllowAllMigrations).unwrap();

        let conn = database.connection();
        let description: String = conn
            .query_row("SELECT description FROM tickets WHERE id = 1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert!(description.contains("Login flow"));
        assert!(description.contains(PROBLEM));

        let comment = load_migrated_comment(&conn, 1);
        assert_eq!(comment.text, "Edited note");
        assert_eq!(comment.author, COMMENT_AUTHOR);
        assert_eq!(comment.created_at, COMMENT_CREATED);
        assert_eq!(comment.edited_at.as_deref(), Some(COMMENT_EDITED));
        assert!(comment.edited);

        let attachments = dir.path().join("attachments");
        std::fs::create_dir_all(&attachments).unwrap();
        let idempotency = Arc::new(SqliteIdempotencyStore::new(
            &database,
            RetentionPolicy::keep_most_recent(NonZeroU32::new(100).unwrap()),
        ));
        let mut core = Core::new(exposed_operations(), idempotency, Arc::new(NoopEventSink));
        register_operator_stack(&mut core, &database, &attachments);
        let session = OperatorSession::new(Arc::new(core));

        let blocker: IssueRecord = serde_json::from_value(
            session
                .call(
                    "issue.get",
                    &serde_json::to_value(IssueGetQuery {
                        identifier: "CORE-1".into(),
                    })
                    .unwrap(),
                )
                .unwrap(),
        )
        .unwrap();
        let waiting: IssueRecord = serde_json::from_value(
            session
                .call(
                    "issue.get",
                    &serde_json::to_value(IssueGetQuery {
                        identifier: "CORE-2".into(),
                    })
                    .unwrap(),
                )
                .unwrap(),
        )
        .unwrap();

        assert!(
            blocker
                .relations
                .iter()
                .any(|relation| relation.kind == "blocks" && relation.identifier == "CORE-2")
        );
        assert!(
            waiting
                .relations
                .iter()
                .any(|relation| relation.kind == "blocked-by" && relation.identifier == "CORE-1")
        );
        assert_eq!(blocker.title, "Login flow");
        assert!(
            waiting
                .description
                .contains("Vendor API quota holds the rollout"),
            "legacy external blocker prose should land in the issue description"
        );
        assert!(
            !blocker.state_history.is_empty(),
            "state history should be backfilled from legacy transitions"
        );
        assert_eq!(blocker.state_history.len(), 3);
        assert_eq!(blocker.state_history[0].state, "Draft");
        assert_eq!(
            blocker.state_history[0].started_at,
            "2019-01-01T00:00:00.000000Z"
        );
        assert_eq!(
            blocker.state_history[0].ended_at.as_deref(),
            Some("2019-01-01T12:00:00.000000Z")
        );
        assert_eq!(blocker.state_history[1].state, "Active");
        assert_eq!(
            blocker.state_history[1].started_at,
            "2019-01-01T12:00:00.000000Z"
        );
        assert_eq!(
            blocker.state_history[1].ended_at.as_deref(),
            Some("2019-01-02T12:00:00.000000Z")
        );
        assert_eq!(blocker.state_history[2].state, "Done");
        assert_eq!(
            blocker.state_history[2].started_at,
            "2019-01-02T12:00:00.000000Z"
        );
        assert!(blocker.state_history[2].ended_at.is_none());

        let comment_store = SqliteCommentStore::new(&database);
        let revisions = comment_store
            .revisions(CommentId::new(1))
            .expect("comment history loads");
        assert_eq!(revisions.0.text, "Edited note");
        assert_eq!(revisions.1.len(), 2);
        assert_eq!(revisions.1[0].recorded_at, COMMENT_CREATED);
        assert_eq!(revisions.1[1].recorded_at, COMMENT_EDITED);
    }
    #[test]
    fn migration_converts_legacy_datetime_due_dates_to_calendar_dates() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("kanban.sqlite");
        let mut database = Database::open(&path).unwrap();
        migrations::apply_through(&database.connection(), 52).unwrap();
        seed_project(&database);
        {
            let conn = database.connection();
            seed_legacy_task(
                &conn,
                1,
                "active",
                Some("2026-11-15T17:00:00.000Z"),
                "2026-11-03T21:38:37.985Z",
            );
            seed_legacy_transition(&conn, 1, "draft", "active", "2026-11-03T21:39:55.127Z");
            seed_legacy_task(&conn, 2, "parked", None, "2026-11-03T21:38:37.988Z");
        }
        database.migrate(&AllowAllMigrations).unwrap();

        let conn = database.connection();
        assert_eq!(migrated_due_date(&conn, 1), "2026-11-15");
        assert_eq!(migrated_due_date(&conn, 2), "");
    }

    #[test]
    fn migration_backfills_terminal_timestamps_from_state_history() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("kanban.sqlite");
        let mut database = Database::open(&path).unwrap();
        migrations::apply_through(&database.connection(), 52).unwrap();
        seed_project(&database);
        {
            let conn = database.connection();
            seed_legacy_task(&conn, 1, "done", None, "2026-11-03T21:38:37.981Z");
            seed_legacy_transition(&conn, 1, "draft", "ready", "2026-11-03T21:38:46.601Z");
            seed_legacy_transition(&conn, 1, "ready", "active", "2026-11-03T21:38:50.957Z");
            seed_legacy_transition(&conn, 1, "active", "in_review", "2026-11-03T21:38:56.423Z");
            seed_legacy_transition(
                &conn,
                1,
                "in_review",
                "approved",
                "2026-11-03T21:38:56.427Z",
            );
            seed_legacy_transition(&conn, 1, "approved", "landing", "2026-11-03T21:39:00.402Z");
            seed_legacy_transition(&conn, 1, "landing", "done", "2026-11-03T21:39:00.405Z");

            seed_legacy_bug(&conn, 2, "cancelled", None, "2026-11-03T21:38:37.996Z");
            seed_legacy_transition(&conn, 2, "draft", "cancelled", "2026-11-03T21:39:55.119Z");

            seed_legacy_task(&conn, 3, "parked", None, "2026-11-03T21:38:37.988Z");
            seed_legacy_transition(&conn, 3, "draft", "parked", "2026-11-03T21:38:46.608Z");

            seed_legacy_task(&conn, 4, "active", None, "2026-11-03T21:38:37.985Z");
            seed_legacy_transition(&conn, 4, "draft", "done", "2026-11-03T21:39:10.000Z");
            seed_legacy_transition(&conn, 4, "done", "active", "2026-11-03T21:39:20.000Z");

            seed_legacy_task(&conn, 5, "done", None, "2026-11-03T21:38:37.989Z");
            seed_legacy_transition(&conn, 5, "draft", "done", "2026-11-03T21:39:30.000Z");
            seed_legacy_transition(&conn, 5, "done", "active", "2026-11-03T21:39:40.000Z");
            seed_legacy_transition(&conn, 5, "active", "done", "2026-11-03T21:39:50.000Z");
        }
        database.migrate(&AllowAllMigrations).unwrap();

        let conn = database.connection();
        let (started, completed, canceled) = migrated_lifecycle_timestamps(&conn, 1);
        assert_eq!(started.as_deref(), Some("2026-11-03T21:38:50.957Z"));
        assert_eq!(
            completed.as_deref(),
            Some("2026-11-03T21:39:00.405Z"),
            "the last entry into the completed state completes the issue"
        );
        assert_eq!(canceled, None);

        let (started, completed, canceled) = migrated_lifecycle_timestamps(&conn, 2);
        assert_eq!(started, None);
        assert_eq!(completed, None);
        assert_eq!(
            canceled.as_deref(),
            Some("2026-11-03T21:39:55.119Z"),
            "the last entry into the canceled state cancels the issue"
        );

        let (_, completed, canceled) = migrated_lifecycle_timestamps(&conn, 3);
        assert_eq!(completed, None, "a parked issue was never terminal");
        assert_eq!(canceled, None);

        let (_, completed, canceled) = migrated_lifecycle_timestamps(&conn, 4);
        assert_eq!(
            completed, None,
            "an issue that left the completed state is no longer completed"
        );
        assert_eq!(canceled, None);

        let (started, completed, canceled) = migrated_lifecycle_timestamps(&conn, 5);
        assert_eq!(started.as_deref(), Some("2026-11-03T21:39:40.000Z"));
        assert_eq!(
            completed.as_deref(),
            Some("2026-11-03T21:39:50.000Z"),
            "an issue that re-entered the completed state is completed by the last entry"
        );
        assert_eq!(canceled, None);
    }

    #[test]
    fn migration_keeps_a_history_row_for_the_current_state() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("kanban.sqlite");
        let mut database = Database::open(&path).unwrap();
        migrations::apply_through(&database.connection(), 52).unwrap();
        seed_project(&database);
        {
            let conn = database.connection();
            seed_legacy_bug(&conn, 1, "draft", None, "2026-11-03T21:38:37.997Z");
            seed_legacy_transition(&conn, 1, "draft", "draft", "2026-11-03T21:40:21.545Z");

            seed_legacy_task(&conn, 2, "parked", None, "2026-11-03T21:38:37.988Z");
            seed_legacy_transition(&conn, 2, "draft", "parked", "2026-11-03T21:38:46.608Z");
            seed_legacy_transition(&conn, 2, "parked", "parked", "2026-11-03T21:40:21.542Z");
        }
        database.migrate(&AllowAllMigrations).unwrap();

        let conn = database.connection();
        let draft_only = migrated_history(&conn, 1);
        assert_eq!(
            draft_only,
            vec![(
                "Draft".to_owned(),
                "2026-11-03T21:38:37.997Z".to_owned(),
                None
            )],
            "an issue that only revisited its current state keeps one open row for it"
        );

        let parked = migrated_history(&conn, 2);
        assert_eq!(
            parked,
            vec![
                (
                    "Draft".to_owned(),
                    "2026-11-03T21:38:37.988Z".to_owned(),
                    Some("2026-11-03T21:38:46.608Z".to_owned())
                ),
                (
                    "Parked".to_owned(),
                    "2026-11-03T21:38:46.608Z".to_owned(),
                    None
                ),
            ],
            "a same-state visit is dropped and the current state stays open"
        );
    }

    #[test]
    fn migration_makes_severity_and_mode_groups_single_select() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("kanban.sqlite");
        let mut database = Database::open(&path).unwrap();
        migrations::apply_through(&database.connection(), 52).unwrap();
        seed_project(&database);
        {
            let conn = database.connection();
            seed_legacy_bug(
                &conn,
                1,
                "draft",
                Some("critical"),
                "2026-11-03T21:38:37.997Z",
            );
            seed_legacy_task(&conn, 2, "active", None, "2026-11-03T21:38:37.985Z");
            conn.execute(
                "UPDATE tickets SET mode = 'agent', subtype = 'investigative' WHERE id = 2",
                [],
            )
            .unwrap();
            seed_legacy_transition(&conn, 2, "draft", "active", "2026-11-03T21:39:55.127Z");
        }
        database.migrate(&AllowAllMigrations).unwrap();

        let conn = database.connection();
        let mut statement = conn
            .prepare("SELECT name, single_select FROM issue_label_groups ORDER BY name")
            .unwrap();
        let groups = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            groups,
            vec![
                ("Mode".to_owned(), 1),
                ("Severity".to_owned(), 1),
                ("Subtype".to_owned(), 0),
                ("Type".to_owned(), 1),
            ],
            "an issue carries at most one severity and one mode, like its type"
        );
    }

    #[test]
    fn repair_migration_converts_earlier_build_datetime_due_dates() {
        let (_dir, mut database) = earlier_build_database();
        database.migrate(&AllowAllMigrations).unwrap();
        assert_eq!(migrated_due_date(&database.connection(), 1), "2026-11-15");
    }

    #[test]
    fn repair_migration_preserves_cleared_due_dates() {
        let (_dir, mut database) = earlier_build_database();
        IssueStoreImpl::new(&database)
            .set_due_date(1, None)
            .unwrap();
        assert_eq!(migrated_due_date(&database.connection(), 1), "");

        database.migrate(&AllowAllMigrations).unwrap();
        assert_eq!(
            migrated_due_date(&database.connection(), 1),
            "",
            "a due date cleared through the issue store stays cleared"
        );
    }
    #[test]
    fn repair_migration_pads_earlier_build_dates_that_parse() {
        let (_dir, mut database) = earlier_build_database();
        let stored_by_earlier_builds = [
            (2_i64, " 2027-01-05"),
            (3, "2027-01- 05"),
            (5, "-0001-01-01"),
            (6, "+10000-01-01"),
            (7, "2027-1-5"),
            (8, "2027- 1- 5"),
        ];
        {
            let conn = database.connection();
            for (ticket_id, raw) in stored_by_earlier_builds {
                conn.execute(
                    "UPDATE tickets SET due_date = ?2 WHERE id = ?1",
                    rusqlite::params![ticket_id, raw],
                )
                .unwrap();
                assert_eq!(migrated_due_date(&conn, ticket_id), raw);
            }
        }

        database.migrate(&AllowAllMigrations).unwrap();

        // Every date that parses as a calendar date is rewritten in
        // its zero-padded spelling and saves back; the odd years are
        // their own canonical spelling, keep their bytes, and still
        // meet the strict validator's refusal on save.
        let expected = [
            (2_i64, " 2027-01-05", "2027-01-05", true),
            (3, "2027-01- 05", "2027-01-05", true),
            (5, "-0001-01-01", "-0001-01-01", false),
            (6, "+10000-01-01", "+10000-01-01", false),
            (7, "2027-1-5", "2027-01-05", true),
            (8, "2027- 1- 5", "2027-01-05", true),
        ];
        for (ticket_id, raw, now_holds, saves_back) in expected {
            assert_eq!(
                migrated_due_date(&database.connection(), ticket_id),
                now_holds,
                "the due date {raw:?} an earlier build stored"
            );
            let outcome = IssueStoreImpl::new(&database).set_due_date(ticket_id, Some(now_holds));
            if saves_back {
                outcome.expect("a zero-padded date saves back");
            } else {
                let refused = outcome.expect_err("the store refuses a non-padded date");
                assert_eq!(refused.code, ErrorCode::InvalidRequest);
                assert_eq!(refused.field.as_deref(), Some("due_date"), "{raw:?}");
            }
        }
    }

    #[test]
    fn repair_migration_backfills_earlier_build_terminal_stamps() {
        let (_dir, mut database) = earlier_build_database();
        database.migrate(&AllowAllMigrations).unwrap();

        let conn = database.connection();
        let (_, completed, canceled) = migrated_lifecycle_timestamps(&conn, 2);
        assert_eq!(completed.as_deref(), Some("2026-11-03T21:39:00.405Z"));
        assert_eq!(canceled, None);
        let (_, completed, canceled) = migrated_lifecycle_timestamps(&conn, 3);
        assert_eq!(completed, None);
        assert_eq!(canceled.as_deref(), Some("2026-11-03T21:39:55.119Z"));
        let (_, completed, canceled) = migrated_lifecycle_timestamps(&conn, 1);
        assert_eq!(completed, None, "an active issue is not terminal");
        assert_eq!(canceled, None);
    }

    #[test]
    fn repair_migration_clears_terminal_stamps_for_reopened_issues() {
        let (_dir, mut database) = earlier_build_database();
        database.migrate(&AllowAllMigrations).unwrap();

        let conn = database.connection();
        let (_, completed, canceled) = migrated_lifecycle_timestamps(&conn, 6);
        assert_eq!(
            completed, None,
            "an issue that left the completed state keeps no completed stamp"
        );
        assert_eq!(canceled, None);
        let (_, completed, canceled) = migrated_lifecycle_timestamps(&conn, 8);
        assert_eq!(completed, None);
        assert_eq!(
            canceled, None,
            "an issue that left the canceled state keeps no canceled stamp"
        );
    }

    #[test]
    fn repair_migration_restamps_reentered_terminal_issues_from_last_entry() {
        let (_dir, mut database) = earlier_build_database();
        database.migrate(&AllowAllMigrations).unwrap();

        let conn = database.connection();
        let (_, completed, _) = migrated_lifecycle_timestamps(&conn, 7);
        assert_eq!(
            completed.as_deref(),
            Some("2026-11-03T21:46:30.000Z"),
            "an issue that re-entered the completed state is completed by the last entry"
        );
        let (_, _, canceled) = migrated_lifecycle_timestamps(&conn, 9);
        assert_eq!(
            canceled.as_deref(),
            Some("2026-11-03T21:48:30.000Z"),
            "an issue that re-entered the canceled state is canceled by the last entry"
        );
    }

    #[test]
    fn repair_migration_restores_earlier_build_current_state_rows() {
        let (_dir, mut database) = earlier_build_database();
        database.migrate(&AllowAllMigrations).unwrap();
        assert_eq!(
            migrated_history(&database.connection(), 4),
            vec![(
                "Draft".to_owned(),
                "2026-11-03T21:38:37.997Z".to_owned(),
                None
            )]
        );
    }

    #[test]
    fn repair_migration_reopens_a_closed_current_state_row() {
        let (_dir, mut database) = earlier_build_database();
        database.migrate(&AllowAllMigrations).unwrap();
        assert_eq!(
            migrated_history(&database.connection(), 10),
            vec![
                (
                    "Draft".to_owned(),
                    "2026-11-03T21:38:37.988Z".to_owned(),
                    Some("2026-11-03T21:38:46.608Z".to_owned())
                ),
                (
                    "Parked".to_owned(),
                    "2026-11-03T21:38:46.608Z".to_owned(),
                    None
                ),
            ],
            "the closed row for the current state reopens instead of staying shut"
        );
    }

    #[test]
    fn repair_migration_makes_earlier_build_groups_single_select() {
        let (_dir, mut database) = earlier_build_database();
        database.migrate(&AllowAllMigrations).unwrap();

        let conn = database.connection();
        let mut statement = conn
            .prepare("SELECT name, single_select FROM issue_label_groups ORDER BY name")
            .unwrap();
        let groups = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            groups,
            vec![
                ("Mode".to_owned(), 1),
                ("Severity".to_owned(), 1),
                ("Subtype".to_owned(), 0),
                ("Type".to_owned(), 1),
            ]
        );
    }

    #[test]
    fn repair_migration_reapplies_are_idempotent() {
        let (_dir, mut database) = earlier_build_database();
        database.migrate(&AllowAllMigrations).unwrap();
        {
            let conn = database.connection();
            apply_linear_issue_fidelity(&conn).unwrap();
            apply_linear_issue_fidelity(&conn).unwrap();
        }
        database.migrate(&AllowAllMigrations).unwrap();

        let conn = database.connection();
        assert_eq!(migrated_due_date(&conn, 1), "2026-11-15");
        assert_eq!(
            migrated_lifecycle_timestamps(&conn, 2).1.as_deref(),
            Some("2026-11-03T21:39:00.405Z")
        );
        assert_eq!(migrated_history(&conn, 4).len(), 1);
        let mode_select: i64 = conn
            .query_row(
                "SELECT single_select FROM issue_label_groups WHERE name = 'Mode'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(mode_select, 1);
    }

    #[test]
    fn migrated_due_date_round_trips_through_issue_save() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("kanban.sqlite");
        let mut database = Database::open(&path).unwrap();
        migrations::apply_through(&database.connection(), 52).unwrap();
        seed_project(&database);
        {
            let conn = database.connection();
            seed_legacy_task(
                &conn,
                1,
                "active",
                Some("2026-11-10T17:00:00.000Z"),
                "2026-11-03T21:38:37.985Z",
            );
            seed_legacy_transition(&conn, 1, "draft", "active", "2026-11-03T21:39:55.127Z");
        }
        database.migrate(&AllowAllMigrations).unwrap();

        let attachments = dir.path().join("attachments");
        std::fs::create_dir_all(&attachments).unwrap();
        let idempotency = Arc::new(SqliteIdempotencyStore::new(
            &database,
            RetentionPolicy::keep_most_recent(NonZeroU32::new(100).unwrap()),
        ));
        let mut core = Core::new(exposed_operations(), idempotency, Arc::new(NoopEventSink));
        register_operator_stack(&mut core, &database, &attachments);
        let session = OperatorSession::new(Arc::new(core));

        let read: IssueRecord = serde_json::from_value(
            session
                .call(
                    "issue.get",
                    &serde_json::to_value(IssueGetQuery {
                        identifier: "CORE-1".into(),
                    })
                    .unwrap(),
                )
                .expect("issue.get succeeds"),
        )
        .unwrap();
        assert_eq!(read.due_date.as_deref(), Some("2026-11-10"));

        session
            .call(
                "issue.save",
                &json!({
                    "mutation": {
                        "optimistic_version": read.version,
                        "idempotency_key": "migrated-due-round-trip"
                    },
                    "identifier": "CORE-1",
                    "due_date": read.due_date,
                }),
            )
            .expect("a migrated due date saves back unchanged");
    }

    /// Prove migration 0061 on a database an earlier build left with loose
    /// issue dates: every stored due date and milestone target date that
    /// parses as a calendar date is rewritten zero-padded, the retired
    /// in-band no-date marker becomes the none value, and anything
    /// unparseable keeps its bytes.
    #[test]
    fn migration_0061_pads_stored_issue_dates_and_retires_the_marker() {
        let dir = tempfile::TempDir::new().expect("a scratch directory is available");
        let path = dir.path().join("kanban.sqlite");
        let mut database = Database::open(&path).expect("the scratch database opens");
        migrations::apply_through(&database.connection(), 60).expect("schema 60 lands");
        seed_project(&database);
        {
            let conn = database.connection();
            for (id, state, due) in [
                (1i64, "done", "2026-1-5"),
                (2i64, "draft", "2099-12-31"),
                (3i64, "draft", " 2026-11-05 "),
                (4i64, "draft", "not a calendar date"),
                (5i64, "draft", "2026-02-30"),
                (6i64, "draft", "2026-11-05Z"),
            ] {
                conn.execute(
                    "INSERT INTO tickets (
                 id, project_id, number, kind, priority, state, title, criteria,
                 subtype, mode, completion, version, due_date
             ) VALUES (?1, 1, ?1, 'task', 'normal', ?2, ?3, '[]',
                       'operational', 'human', '[\"Tracked\"]', 1, ?4)",
                    rusqlite::params![id, state, format!("Ticket {id}"), due],
                )
                .unwrap();
            }
            conn.execute(
                "INSERT INTO tickets (
             id, project_id, number, kind, priority, state, title, criteria,
             subtype, mode, completion, version
         ) VALUES (7, 1, 7, 'task', 'normal', 'draft', 'Unmarked ticket', '[]',
                   'operational', 'human', '[\"Tracked\"]', 1)",
                [],
            )
            .unwrap();
            for (id, target) in [(1i64, "2027-2-3"), (2i64, "2027-03-04"), (3i64, "soon")] {
                conn.execute(
                    "INSERT INTO issue_milestones (id, project_id, name, sort_order, target_date)
             VALUES (?1, 1, ?2, ?1, ?3)",
                    rusqlite::params![id, format!("M{id}"), target],
                )
                .unwrap();
            }
            conn.execute(
                "INSERT INTO issue_milestones (id, project_id, name, sort_order)
         VALUES (4, 1, 'M4', 4)",
                [],
            )
            .unwrap();
        }

        database
            .migrate(&AllowAllMigrations)
            .expect("migration 0061 applies");

        let conn = database.connection();
        let due: Vec<(i64, String)> = conn
            .prepare("SELECT id, due_date FROM tickets ORDER BY id")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            due,
            vec![
                (1, "2026-01-05".to_owned()),
                (2, String::new()),
                (3, "2026-11-05".to_owned()),
                (4, "not a calendar date".to_owned()),
                (5, "2026-02-30".to_owned()),
                (6, "2026-11-05Z".to_owned()),
                (7, String::new()),
            ],
            "loose dates pad, the marker retires, unparseable text stands"
        );
        let targets: Vec<(i64, Option<String>)> = conn
            .prepare("SELECT id, target_date FROM issue_milestones ORDER BY id")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            targets,
            vec![
                (1, Some("2027-02-03".to_owned())),
                (2, Some("2027-03-04".to_owned())),
                (3, Some("soon".to_owned())),
                (4, None),
            ],
            "milestone target dates pad the same way, without a marker"
        );
    }

    #[test]
    fn copied_fixture_tasks_gain_a_checklist_on_upgrade_and_repair() {
        let Ok(source) = std::env::var("KANBAN_OLD_FIXTURE") else {
            return;
        };
        let source = std::path::PathBuf::from(source);
        assert!(source.is_file(), "the copied fixture is readable");
        let fresh = tempfile::TempDir::new().unwrap();
        let fresh_path = fresh.path().join("kanban.sqlite");
        std::fs::copy(&source, &fresh_path).unwrap();
        let fresh_path = fresh.path().join("kanban.sqlite");
        std::fs::copy(&source, &fresh_path).unwrap();
        for suffix in ["-wal", "-shm"] {
            let sidecar = std::path::PathBuf::from(format!("{}{suffix}", source.display()));
            if sidecar.is_file() {
                std::fs::copy(
                    &sidecar,
                    fresh.path().join(format!("kanban.sqlite{suffix}")),
                )
                .unwrap();
            }
        }
        let mut database = Database::open(&fresh_path).unwrap();
        database
            .migrate(&AllowAllMigrations)
            .expect("a schema 52 copy upgrades");
        let tasks = task_descriptions(&database);
        assert!(!tasks.is_empty(), "the fixture holds tasks");
        for (id, completion, description) in &tasks {
            let outcomes: Vec<String> = serde_json::from_str(completion).unwrap();
            for outcome in &outcomes {
                assert!(
                    description.contains(&format!("- [ ] {outcome}"))
                        || description.contains(&format!("- [x] {outcome}")),
                    "task {id} keeps criterion `{outcome}` in {description}"
                );
            }
        }
        let upgraded = tasks.clone();
        eprintln!("upgraded fixture tasks: {upgraded:?}");
        let conn = database.connection();
        conn.execute("DELETE FROM schema_migrations WHERE version = 64", [])
            .unwrap();
        for (id, _, description) in &upgraded {
            let stripped = description
                .lines()
                .filter(|line| !line.starts_with("- ["))
                .collect::<Vec<_>>()
                .join("\n");
            conn.execute(
                "UPDATE tickets SET description = ?2 WHERE id = ?1",
                rusqlite::params![id, stripped],
            )
            .unwrap();
        }
        drop(conn);
        database.migrate(&AllowAllMigrations).expect(
            "migration 0064 repairs checklists on a database already at the previous schema",
        );
        let repaired = task_descriptions(&database);
        eprintln!("repaired by migration 0064: {repaired:?}");
        for (id, completion, description) in &repaired {
            let outcomes: Vec<String> = serde_json::from_str(completion).unwrap();
            for outcome in &outcomes {
                assert!(
                    description.contains(&format!("- [ ] {outcome}"))
                        || description.contains(&format!("- [x] {outcome}")),
                    "repair restores criterion `{outcome}` on task {id}"
                );
            }
        }
        database
            .migrate(&AllowAllMigrations)
            .expect("a second migrate leaves the repaired checklist alone");
        assert_eq!(
            task_descriptions(&database),
            repaired,
            "a second repair does not append the checklist again"
        );
    }

    fn task_descriptions(database: &Database) -> Vec<(i64, String, String)> {
        let conn = database.connection();
        let mut statement = conn
            .prepare(
                "SELECT id, completion, description FROM tickets WHERE kind = 'task' ORDER BY id",
            )
            .unwrap();
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    #[test]
    fn a_satisfied_criterion_lands_as_a_checked_item() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("kanban.sqlite");
        let mut database = Database::open(&path).unwrap();
        migrations::apply_through(&database.connection(), 52).unwrap();
        seed_project(&database);
        {
            let conn = database.connection();
            seed_legacy_task(&conn, 1, "active", None, "2026-11-03T21:38:37.985Z");
            conn.execute(
                "UPDATE tickets SET completion = ?1 WHERE id = 1",
                [r#"["Ship the build","Archive the notes"]"#],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO criterion_bindings (
                     ticket_id, criterion_index, kind, evidence_id, tip, review, satisfied, void
                 ) VALUES (1, 1, 'task', 0, 'notes archived', 'validated', 1, 0)",
                [],
            )
            .unwrap();
        }
        database
            .migrate(&AllowAllMigrations)
            .expect("a schema 52 database upgrades");
        let description = task_descriptions(&database)
            .into_iter()
            .find(|row| row.0 == 1)
            .expect("the task is present")
            .2;
        assert!(
            description.contains("- [ ] Ship the build"),
            "an unmet criterion stays open: {description}"
        );
        assert!(
            description.contains("- [x] Archive the notes"),
            "a satisfied criterion is checked: {description}"
        );

        let stripped = description
            .lines()
            .filter(|line| !line.starts_with("- ["))
            .collect::<Vec<_>>()
            .join("\n");
        {
            let conn = database.connection();
            conn.execute(
                "UPDATE tickets SET description = ?1 WHERE id = 1",
                [stripped],
            )
            .unwrap();
            conn.execute("DELETE FROM schema_migrations WHERE version = 64", [])
                .unwrap();
        }
        database.migrate(&AllowAllMigrations).expect(
            "the repair migration restores a checked item on a database already at the previous schema",
        );
        let repaired = task_descriptions(&database)
            .into_iter()
            .find(|row| row.0 == 1)
            .expect("the task is present")
            .2;
        assert!(
            repaired.contains("- [x] Archive the notes"),
            "the repair checks the satisfied criterion: {repaired}"
        );
        assert!(
            repaired.contains("- [ ] Ship the build"),
            "the repair leaves the unmet criterion open: {repaired}"
        );
        database
            .migrate(&AllowAllMigrations)
            .expect("a second repair leaves the checklist alone");
        assert_eq!(
            task_descriptions(&database)
                .into_iter()
                .find(|row| row.0 == 1)
                .expect("the task is present")
                .2,
            repaired
        );
    }

    #[test]
    fn a_checklist_already_followed_by_prose_is_left_unchanged() {
        let description = "## Title\n\nRotate local backups\n\n- [ ] Rotate local backups done\n\nLater note stays.\n";
        let checklist = "- [ ] Rotate local backups done";
        assert_eq!(append_checklist(description, checklist), description);
        let spaced = "Keep the trailing spaces  ";
        let appended = append_checklist(spaced, checklist);
        assert!(
            appended.starts_with(spaced),
            "appending must not rewrite the existing text: {appended:?}"
        );
        assert!(appended.contains(checklist));
    }
    #[test]
    fn a_user_checked_item_is_not_replaced_by_an_unchecked_copy() {
        let description =
            "Existing prose  \n\n- [x] Issue tracked\n\nOperator checked this item.\n";
        let checklist = "- [ ] Issue tracked";
        assert_eq!(
            append_checklist(description, checklist),
            description,
            "an edited mark is still the checklist, so repair must not append"
        );
    }
    #[test]
    fn stray_block_edges_join_the_dependency_graph_and_related_links_stay() {
        let dir = tempfile::TempDir::new().expect("a scratch directory is available");
        let mut database = Database::open(&dir.path().join("kanban.sqlite")).unwrap();
        migrations::apply_through(&database.connection(), 62).expect("the prior schema applies");
        seed_project(&database);
        {
            let conn = database.connection();
            let project_id: i64 = conn
                .query_row("SELECT id FROM projects", [], |row| row.get(0))
                .unwrap();
            for (id, state, title) in [
                (1i64, "draft", "Open blocker"),
                (2, "draft", "Waiting work"),
                (3, "done", "Landed blocker"),
                (4, "draft", "Neighbour"),
                (5, "draft", "Second open blocker"),
            ] {
                conn.execute(
                    "INSERT INTO tickets (
                         id, project_id, number, kind, priority, state, title, criteria,
                         subtype, mode, completion, version
                     ) VALUES (?1, ?2, ?1, 'task', 'normal', ?3, ?4, '[]',
                               'operational', 'human', '[\"Tracked\"]', 1)",
                    rusqlite::params![id, project_id, state, title],
                )
                .unwrap();
            }
            conn.execute_batch(
                "INSERT INTO issue_relations (from_ticket_id, to_ticket_id, kind) VALUES
                     (1, 2, 'blocks'),
                     (5, 4, 'blocks'),
                     (3, 2, 'blocks'),
                     (2, 4, 'related');
                 INSERT INTO ticket_dependencies (from_ticket, to_ticket) VALUES (3, 2);",
            )
            .unwrap();
        }

        assert_eq!(
            dependency_edges(&database),
            BTreeSet::from([(3, 2)]),
            "a block recorded only as a relation is not yet a dependency"
        );
        assert!(
            blocking_ticket_ids(&database, 2).is_empty(),
            "readiness ignores a block that has not joined the dependency graph"
        );
        assert!(blocking_ticket_ids(&database, 4).is_empty());
        assert_eq!(
            listed_relations(&database, 2),
            vec![
                ("blocked-by".to_owned(), "CORE-3".to_owned()),
                ("related".to_owned(), "CORE-4".to_owned()),
            ]
        );
        assert!(listed_relations(&database, 1).is_empty());
        assert_eq!(
            stored_relations(&database),
            vec![
                (1, 2, "blocks".to_owned()),
                (3, 2, "blocks".to_owned()),
                (5, 4, "blocks".to_owned()),
                (2, 4, "related".to_owned()),
            ]
        );

        let report = database
            .migrate(&AllowAllMigrations)
            .expect("pending migrations apply");
        assert_eq!(
            report.applied,
            (63..=crate::migrations::LATEST_SCHEMA_VERSION).collect::<Vec<_>>()
        );

        let moved = BTreeSet::from([(1, 2), (3, 2), (5, 4)]);
        assert_eq!(dependency_edges(&database), moved);
        assert_eq!(
            dependency_row_count(&database),
            3,
            "the repeated block is one edge"
        );
        assert_eq!(
            blocking_ticket_ids(&database, 2),
            vec![1],
            "the moved open blocker holds the waiting issue; the landed one does not"
        );
        assert_eq!(blocking_ticket_ids(&database, 4), vec![5]);
        assert_eq!(
            listed_relations(&database, 1),
            vec![("blocks".to_owned(), "CORE-2".to_owned())]
        );
        assert_eq!(
            listed_relations(&database, 2),
            vec![
                ("blocked-by".to_owned(), "CORE-1".to_owned()),
                ("blocked-by".to_owned(), "CORE-3".to_owned()),
                ("related".to_owned(), "CORE-4".to_owned()),
            ]
        );
        assert_eq!(
            listed_relations(&database, 4),
            vec![
                ("blocked-by".to_owned(), "CORE-5".to_owned()),
                ("related".to_owned(), "CORE-2".to_owned()),
            ]
        );
        assert_eq!(
            listed_relations(&database, 5),
            vec![("blocks".to_owned(), "CORE-4".to_owned())]
        );
        assert_eq!(
            stored_relations(&database),
            vec![(2, 4, "related".to_owned())],
            "block rows leave relations and the related link stays"
        );

        let again = database
            .migrate(&AllowAllMigrations)
            .expect("a repeated migration applies nothing");
        assert!(again.applied.is_empty());
        assert_eq!(dependency_edges(&database), moved);
        assert_eq!(dependency_row_count(&database), 3);
        assert_eq!(
            stored_relations(&database),
            vec![(2, 4, "related".to_owned())]
        );
        assert_eq!(blocking_ticket_ids(&database, 2), vec![1]);
        assert_eq!(blocking_ticket_ids(&database, 4), vec![5]);
    }

    fn dependency_edges(database: &Database) -> BTreeSet<(u64, u64)> {
        SqliteDependencyStore::new(database)
            .list_dependencies()
            .unwrap()
            .into_iter()
            .map(|edge| (edge.from().value(), edge.to().value()))
            .collect()
    }

    fn dependency_row_count(database: &Database) -> i64 {
        database
            .connection()
            .query_row("SELECT COUNT(*) FROM ticket_dependencies", [], |row| {
                row.get(0)
            })
            .unwrap()
    }

    fn blocking_ticket_ids(database: &Database, waiting: u64) -> Vec<u64> {
        let tickets = SqliteTicketStore::new(database);
        let dependencies = SqliteDependencyStore::new(database);
        let ticket = tickets.find(TicketId::new(waiting)).unwrap().unwrap();
        let edges = dependencies.list_dependencies().unwrap();
        let graph = TicketDependencyGraph::restore(edges);
        let mut states = Vec::new();
        for edge in graph.required_by(ticket.id()) {
            let blocking = tickets.find(edge.from()).unwrap().unwrap();
            states.push(DependencyState {
                dependency: edge,
                state: blocking.state(),
            });
        }
        let external = dependencies.blockers_of(ticket.id()).unwrap();
        let readiness = compute_readiness(ReadinessInputs {
            dependencies: &states,
            blockers: &external,
        });
        readiness
            .blocked_by()
            .iter()
            .map(|blocker| match blocker {
                ReadinessBlocker::Ticket { waiting } => waiting.dependency.from().value(),
                ReadinessBlocker::External { .. } => panic!("no external blocker was stored"),
            })
            .collect()
    }

    fn listed_relations(database: &Database, ticket_id: i64) -> Vec<(String, String)> {
        IssueStoreImpl::new(database)
            .list_relations(ticket_id)
            .unwrap()
            .into_iter()
            .map(|row| (row.kind, row.identifier))
            .collect()
    }

    fn stored_relations(database: &Database) -> Vec<(i64, i64, String)> {
        let conn = database.connection();
        let mut statement = conn
            .prepare(
                "SELECT from_ticket_id, to_ticket_id, kind
                 FROM issue_relations
                 ORDER BY kind, from_ticket_id, to_ticket_id",
            )
            .unwrap();
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }
}
