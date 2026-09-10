//! Shell preference operations (KAN-S5-US1, KAN-S5-US3): how the
//! operator keeps the surface arranged — the navigation rail's
//! collapse, and which board columns are collapsed to their rail in
//! each scope. The arrangement is per-operator data in the
//! authoritative store, never browser state, so it survives a reload
//! and a new window. A Saved View owns hidden columns and expanded
//! groups (DR-BP-05); these are the decisions no view owns, and one
//! update replaces the whole record so writing one never drops the
//! other. Nothing here touches workflow: the preferences are
//! presentation, and no Ticket, run, or review reads them.

use std::sync::Arc;

use kanban_domain::{
    BoardColumn as DomainColumn, ProjectId, ShellPreferences, ViewScope as DomainScope,
};
use kanban_dto::{
    ApiError, BoardColumn, ScopedCollapsedColumns, ShellPreferencesQuery, ShellPreferencesRecord,
    ShellPreferencesUpdateRequest, ViewScope,
};
use serde_json::Value;

use crate::dispatch::{Core, QueryHandler, RegistrationError};
use crate::mutation::{CommandHandler, ParsedCommand, parse_payload};

/// The storage port the shell preference operations call through. A
/// shell nobody has rearranged holds no row: reads answer the
/// everyday arrangement at version 0, and the first update inserts.
pub trait ShellPreferenceStore: Send + Sync {
    /// The stored arrangement.
    fn preferences(&self) -> Result<ShellPreferences, ApiError>;
    /// Replace the stored arrangement, answering the record as it now
    /// stands.
    fn replace(&self, preferences: &ShellPreferences) -> Result<ShellPreferences, ApiError>;
}

impl Core {
    /// Register the shell preference operations against `store`.
    pub fn register_shell_preferences(
        &mut self,
        store: Arc<dyn ShellPreferenceStore>,
    ) -> Result<(), RegistrationError> {
        self.register_query(
            "shell.preferences",
            Arc::new(GetShellPreferences {
                store: store.clone(),
            }),
        )?;
        self.register_command(
            "shell.preferences.update",
            Arc::new(UpdateShellPreferences { store }),
        )?;
        Ok(())
    }
}

/// Serves `shell.preferences`.
struct GetShellPreferences {
    store: Arc<dyn ShellPreferenceStore>,
}

impl QueryHandler for GetShellPreferences {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        parse_payload::<ShellPreferencesQuery>(payload)?;
        let record = record_of(&self.store.preferences()?);
        serde_json::to_value(record).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

/// Serves `shell.preferences.update`.
struct UpdateShellPreferences {
    store: Arc<dyn ShellPreferenceStore>,
}

impl CommandHandler for UpdateShellPreferences {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<ShellPreferencesUpdateRequest>(payload)?;
        ParsedCommand::lift("shell", payload)
    }

    fn current_version(&self, _command: &ParsedCommand) -> Result<u64, ApiError> {
        Ok(self.store.preferences()?.version())
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        _effects: &dyn crate::CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: ShellPreferencesUpdateRequest = parse_payload(&command.payload)?;
        let held = self.store.preferences()?;
        // The domain canonicalises both sets, so the same arrangement
        // written twice stores identically.
        let next = ShellPreferences::restore(
            request.rail_open,
            request
                .collapsed_columns
                .into_iter()
                .map(|scoped| {
                    (
                        domain_scope(scoped.scope),
                        scoped.columns.into_iter().map(domain_column).collect(),
                    )
                })
                .collect::<Vec<_>>(),
            held.version() + 1,
        );
        let stored = self.store.replace(&next)?;
        serde_json::to_value(record_of(&stored))
            .map_err(|error| ApiError::internal(&error.to_string()))
    }
}

/// The wire record of one stored arrangement.
fn record_of(preferences: &ShellPreferences) -> ShellPreferencesRecord {
    ShellPreferencesRecord {
        rail_open: preferences.rail_open(),
        collapsed_columns: preferences
            .collapsed()
            .iter()
            .map(|scoped| ScopedCollapsedColumns {
                scope: wire_scope(scoped.scope()),
                columns: scoped.columns().iter().copied().map(wire_column).collect(),
            })
            .collect(),
        version: preferences.version(),
    }
}

fn wire_scope(scope: DomainScope) -> ViewScope {
    match scope {
        DomainScope::Global => ViewScope::Global,
        DomainScope::Project(project) => ViewScope::Project(project.value()),
    }
}

fn domain_scope(scope: ViewScope) -> DomainScope {
    match scope {
        ViewScope::Global => DomainScope::Global,
        ViewScope::Project(project) => DomainScope::Project(ProjectId::new(project)),
    }
}

fn wire_column(column: DomainColumn) -> BoardColumn {
    match column {
        DomainColumn::Draft => BoardColumn::Draft,
        DomainColumn::Backlog => BoardColumn::Backlog,
        DomainColumn::Parked => BoardColumn::Parked,
        DomainColumn::Blocked => BoardColumn::Blocked,
        DomainColumn::Scheduled => BoardColumn::Scheduled,
        DomainColumn::Ready => BoardColumn::Ready,
        DomainColumn::Current => BoardColumn::Current,
        DomainColumn::Review => BoardColumn::Review,
        DomainColumn::Staged => BoardColumn::Staged,
        DomainColumn::Approved => BoardColumn::Approved,
        DomainColumn::Landing => BoardColumn::Landing,
        DomainColumn::Done => BoardColumn::Done,
    }
}

fn domain_column(column: BoardColumn) -> DomainColumn {
    match column {
        BoardColumn::Draft => DomainColumn::Draft,
        BoardColumn::Backlog => DomainColumn::Backlog,
        BoardColumn::Parked => DomainColumn::Parked,
        BoardColumn::Blocked => DomainColumn::Blocked,
        BoardColumn::Scheduled => DomainColumn::Scheduled,
        BoardColumn::Ready => DomainColumn::Ready,
        BoardColumn::Current => DomainColumn::Current,
        BoardColumn::Review => DomainColumn::Review,
        BoardColumn::Staged => DomainColumn::Staged,
        BoardColumn::Approved => DomainColumn::Approved,
        BoardColumn::Landing => DomainColumn::Landing,
        BoardColumn::Done => DomainColumn::Done,
    }
}

#[cfg(test)]
mod shell_preference_operations {
    use std::sync::{Arc, Mutex};

    use kanban_dto::{ApiError, ErrorCode};
    use serde_json::{Value, json};

    use super::{ShellPreferenceStore, ShellPreferences};
    use crate::catalog::exposed_operations;
    use crate::dispatch::Core;
    use crate::events::NoopEventSink;
    use crate::mutation::MemoryIdempotencyStore;

    /// A disposable store standing in for the SQLite one: it holds a
    /// record the same way, and answers the everyday arrangement
    /// until one is written.
    #[derive(Default)]
    struct MemoryShellPreferences {
        held: Mutex<Option<ShellPreferences>>,
    }

    impl ShellPreferenceStore for MemoryShellPreferences {
        fn preferences(&self) -> Result<ShellPreferences, ApiError> {
            Ok(self
                .held
                .lock()
                .expect("the store is not poisoned")
                .clone()
                .unwrap_or_else(ShellPreferences::everyday))
        }

        fn replace(&self, preferences: &ShellPreferences) -> Result<ShellPreferences, ApiError> {
            let mut held = self.held.lock().expect("the store is not poisoned");
            *held = Some(preferences.clone());
            Ok(preferences.clone())
        }
    }

    fn harness() -> Core {
        let mut core = Core::new(
            exposed_operations(),
            Arc::new(MemoryIdempotencyStore::new()),
            Arc::new(NoopEventSink),
        );
        core.register_shell_preferences(Arc::new(MemoryShellPreferences::default()))
            .expect("the shell preference operations register");
        core
    }

    fn update(body: Value, version: u64, key: &str) -> Value {
        let mut request = json!({
            "mutation": { "optimistic_version": version, "idempotency_key": key },
        });
        let object = request.as_object_mut().expect("the command is an object");
        for (field, value) in body.as_object().expect("the body is an object") {
            object.insert(field.clone(), value.clone());
        }
        request
    }

    #[test]
    fn a_shell_nobody_has_arranged_reads_the_everyday_arrangement() {
        let core = harness();

        let record = core
            .query("shell.preferences", &json!({}))
            .expect("the preferences serve");

        assert_eq!(
            record,
            json!({ "rail_open": true, "collapsed_columns": [], "version": 0 })
        );
    }

    #[test]
    fn an_arrangement_written_through_the_command_is_what_the_next_read_answers() {
        let core = harness();

        let written = core
            .command(
                "shell.preferences.update",
                &update(
                    json!({
                        "rail_open": false,
                        "collapsed_columns": [
                            { "scope": { "project": 2 }, "columns": ["ready", "backlog"] },
                        ],
                    }),
                    0,
                    "key-arrange",
                ),
            )
            .expect("the arrangement writes");

        assert_eq!(
            written,
            json!({
                "rail_open": false,
                "collapsed_columns": [
                    { "scope": { "project": 2 }, "columns": ["backlog", "ready"] },
                ],
                "version": 1,
            })
        );
        assert_eq!(
            core.query("shell.preferences", &json!({}))
                .expect("the preferences serve"),
            written,
            "a fresh read answers what was written"
        );
    }

    #[test]
    fn a_stale_arrangement_is_refused_and_the_stored_one_stands() {
        let core = harness();
        core.command(
            "shell.preferences.update",
            &update(json!({ "rail_open": false }), 0, "key-first"),
        )
        .expect("the first arrangement writes");

        let error = core
            .command(
                "shell.preferences.update",
                &update(json!({ "rail_open": true }), 0, "key-stale"),
            )
            .expect_err("a write against a spent version is refused");

        assert_eq!(error.code, ErrorCode::StaleVersion);
        assert_eq!(
            core.query("shell.preferences", &json!({}))
                .expect("the preferences serve")["rail_open"],
            json!(false),
            "the refusal left the arrangement alone"
        );
    }

    #[test]
    fn an_update_leaving_a_scope_out_clears_what_that_scope_kept_collapsed() {
        let core = harness();
        core.command(
            "shell.preferences.update",
            &update(
                json!({
                    "rail_open": true,
                    "collapsed_columns": [{ "scope": "global", "columns": ["review"] }],
                }),
                0,
                "key-collapse",
            ),
        )
        .expect("the arrangement writes");

        let cleared = core
            .command(
                "shell.preferences.update",
                &update(json!({ "rail_open": true }), 1, "key-expand"),
            )
            .expect("the arrangement writes");

        assert_eq!(cleared["collapsed_columns"], json!([]));
    }
}
