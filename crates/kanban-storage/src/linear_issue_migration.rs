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
            .filter(|(_, from, to)| !from.is_empty() && !to.is_empty())
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
        backfill_issue_descriptions(conn)?;
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
         END;
         UPDATE tickets
         SET due_date = due
         WHERE due IS NOT NULL
           AND due <> ''
           AND due_date = '2099-12-31';",
    )?;
    backfill_issue_descriptions(conn)?;
    backfill_issue_state_history_from_timeline(conn)?;
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
        let description = build_issue_description(
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

pub(crate) fn apply_linear_issue_relaxation_in_span(span: &WriteSpan<'_>) -> rusqlite::Result<()> {
    apply_linear_issue_relaxation(span)
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;
    use std::sync::Arc;

    use kanban_app::catalog::exposed_operations;
    use kanban_app::events::NoopEventSink;
    use kanban_app::{CommentStore, Core, IssueStore, OperatorSession, ProjectStore};
    use kanban_domain::{CommentId, ProjectRegistration};
    use kanban_dto::{
        IssueGetQuery, IssueRecord, TimelineEntityKind, TimelineEntityRef, TimelineEventKind,
    };
    use serde_json::json;

    use super::*;
    use crate::comments::SqliteCommentStore;
    use crate::db::Database;
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
                 1, 1, 1, 'implementation', 'normal', 'draft', 1, NULL, 'Login flow',
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
        core.register_issue_operator(tickets, projects.clone(), specs, issues, comments.clone())
            .unwrap();
        core.register_comments(comments, projects).unwrap();
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
            !blocker.state_history.is_empty(),
            "state history should be backfilled from legacy transitions"
        );

        let comment_store = SqliteCommentStore::new(&database);
        let revisions = comment_store
            .revisions(CommentId::new(1))
            .expect("comment history loads");
        assert_eq!(revisions.0.text, "Edited note");
        assert_eq!(revisions.1.len(), 2);
        assert_eq!(revisions.1[0].recorded_at, COMMENT_CREATED);
        assert_eq!(revisions.1[1].recorded_at, COMMENT_EDITED);
    }
}
