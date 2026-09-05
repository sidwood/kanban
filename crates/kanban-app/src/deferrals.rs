//! Deferral commands and the list query (KAN-S2-US3, DR-AE-03).
//! Deferrals are immutable once recorded; supersession appends a new
//! record referencing the original.

use std::sync::Arc;

use kanban_domain::{Deferral, DeferralDraft, DeferralId, DeferralReason};
use kanban_dto::{
    ApiError, DeferralIdentity, DeferralListQuery, DeferralListResponse, DeferralRecord,
    DeferralRecordRequest, DeferralSupersedeRequest,
};
use serde_json::{Value, json};

use crate::dispatch::{Core, QueryHandler, RegistrationError};
use crate::event_catalog::event_descriptor;
use crate::events::emit_catalogued;
use crate::mutation::{CommandEffects, CommandHandler, ParsedCommand, parse_payload};
use crate::project::{ProjectStore, resolve_project};
use crate::timeline::TimelineFacts;

/// The storage port deferral commands call through.
pub trait DeferralStore: Send + Sync {
    /// Insert one immutable deferral and its timeline append.
    fn insert(&self, draft: &DeferralDraft, facts: TimelineFacts) -> Result<Deferral, ApiError>;
    /// Load one deferral, if it exists in the project.
    fn find(&self, project_id: u64, id: DeferralId) -> Result<Option<Deferral>, ApiError>;
    /// List every deferral for a project, superseded originals included.
    fn list(&self, query: &DeferralListQuery) -> Result<Vec<Deferral>, ApiError>;
}

impl Core {
    /// Register the deferral operations against `store`, resolving
    /// their Project through `projects`.
    pub fn register_deferrals(
        &mut self,
        store: Arc<dyn DeferralStore>,
        projects: Arc<dyn ProjectStore>,
    ) -> Result<(), RegistrationError> {
        self.register_command(
            "deferral.record",
            Arc::new(RecordDeferral {
                store: store.clone(),
                projects: projects.clone(),
            }),
        )?;
        self.register_command(
            "deferral.supersede",
            Arc::new(SupersedeDeferral {
                store: store.clone(),
                projects: projects.clone(),
            }),
        )?;
        self.register_query("deferral.list", Arc::new(ListDeferrals { store, projects }))?;
        Ok(())
    }
}

struct RecordDeferral {
    store: Arc<dyn DeferralStore>,
    projects: Arc<dyn ProjectStore>,
}

impl CommandHandler for RecordDeferral {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<DeferralRecordRequest>(payload)?;
        ParsedCommand::lift("deferral", payload)
    }

    fn current_version(&self, _command: &ParsedCommand) -> Result<u64, ApiError> {
        Ok(0)
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: DeferralRecordRequest = parse_payload(&command.payload)?;
        let project = resolve_project(self.projects.as_ref(), request.project_id)?;
        let reason = DeferralReason::new(&request.reason)
            .map_err(|error| ApiError::invalid_request(&error.to_string()))?;
        let draft = Deferral::record(project.id().value(), &request.finding_id, reason)
            .map_err(|error| ApiError::invalid_request(&error.to_string()))?;
        let deferral = self.store.insert(
            &draft,
            TimelineFacts {
                kind: kanban_dto::TimelineEventKind::Deferral,
                facts: json!({
                    "finding_id": draft.finding_id,
                    "reason": draft.reason.as_str(),
                }),
            },
        )?;
        announce(effects, event_descriptor("deferral.recorded"), &deferral);
        serde_json::to_value(encode_record(&deferral))
            .map_err(|error| ApiError::internal(&error.to_string()))
    }
}

struct SupersedeDeferral {
    store: Arc<dyn DeferralStore>,
    projects: Arc<dyn ProjectStore>,
}

impl CommandHandler for SupersedeDeferral {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<DeferralSupersedeRequest>(payload)?;
        ParsedCommand::lift("deferral", payload)
    }

    fn current_version(&self, _command: &ParsedCommand) -> Result<u64, ApiError> {
        Ok(0)
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: DeferralSupersedeRequest = parse_payload(&command.payload)?;
        let project = resolve_project(self.projects.as_ref(), request.project_id)?;
        let original = load(
            self.store.as_ref(),
            project.id().value(),
            request.deferral_id,
        )?;
        let reason = DeferralReason::new(&request.reason)
            .map_err(|error| ApiError::invalid_request(&error.to_string()))?;
        let draft = original.supersede(reason);
        let deferral = self.store.insert(
            &draft,
            TimelineFacts {
                kind: kanban_dto::TimelineEventKind::Deferral,
                facts: json!({
                    "finding_id": draft.finding_id,
                    "reason": draft.reason.as_str(),
                    "supersedes_id": original.id().value(),
                }),
            },
        )?;
        announce(effects, event_descriptor("deferral.superseded"), &deferral);
        serde_json::to_value(encode_record(&deferral))
            .map_err(|error| ApiError::internal(&error.to_string()))
    }
}

struct ListDeferrals {
    store: Arc<dyn DeferralStore>,
    projects: Arc<dyn ProjectStore>,
}

impl QueryHandler for ListDeferrals {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        let query = parse_payload::<DeferralListQuery>(payload)?;
        // Resolution refuses the query before any row is read, keeping
        // the list surface honest about the Project it names.
        resolve_project(self.projects.as_ref(), query.project_id)?;
        let deferrals = self.store.list(&query)?;
        let response = DeferralListResponse {
            deferrals: deferrals.iter().map(encode_record).collect(),
        };
        serde_json::to_value(response).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

fn load(
    store: &dyn DeferralStore,
    project_id: u64,
    deferral_id: u64,
) -> Result<Deferral, ApiError> {
    store
        .find(project_id, DeferralId::new(deferral_id))?
        .ok_or_else(|| ApiError::not_found(&format!("deferral {deferral_id}")))
}

fn encode_record(deferral: &Deferral) -> DeferralRecord {
    DeferralRecord {
        id: deferral.id().value(),
        project_id: deferral.project_id(),
        finding_id: deferral.finding_id().to_owned(),
        reason: deferral.reason().as_str().to_owned(),
        supersedes_id: deferral.supersedes().map(|id| id.value()),
        recorded_at: deferral.recorded_at().to_owned(),
    }
}

fn announce(
    effects: &dyn CommandEffects,
    event: &crate::event_catalog::EventDescriptor,
    deferral: &Deferral,
) {
    emit_catalogued(
        effects,
        event,
        &DeferralIdentity {
            id: deferral.id().value(),
        },
    );
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use kanban_dto::{ApiError, DeferralListQuery, ErrorCode};
    use serde_json::json;

    use super::{DeferralStore, ListDeferrals, RecordDeferral, SupersedeDeferral, TimelineFacts};
    use crate::dispatch::QueryHandler;
    use crate::mutation::{CommandHandler, NoopCommandEffects, ParsedCommand};
    use crate::project::testing::{MemoryProjectStore, stored_project};
    use kanban_domain::{Deferral, DeferralDraft, DeferralId};

    #[derive(Default)]
    struct MemoryDeferralStore {
        deferrals: Mutex<Vec<Deferral>>,
    }

    impl MemoryDeferralStore {
        fn insert_draft(&self, draft: DeferralDraft) -> Deferral {
            let mut deferrals = self.deferrals.lock().expect("lock is sound");
            let id = deferrals.len() as u64 + 1;
            let recorded_at = format!("2026-09-04T12:00:{id:02}Z");
            deferrals.push(Deferral::restore(
                DeferralId::new(id),
                draft.project_id,
                draft.finding_id,
                draft.reason,
                draft.supersedes,
                recorded_at,
            ));
            deferrals.last().expect("just pushed").clone()
        }
    }

    impl DeferralStore for MemoryDeferralStore {
        fn insert(
            &self,
            draft: &DeferralDraft,
            _facts: TimelineFacts,
        ) -> Result<Deferral, ApiError> {
            Ok(self.insert_draft(draft.clone()))
        }

        fn find(&self, project_id: u64, id: DeferralId) -> Result<Option<Deferral>, ApiError> {
            let deferrals = self.deferrals.lock().expect("lock is sound");
            Ok(deferrals
                .iter()
                .find(|deferral| deferral.project_id() == project_id && deferral.id() == id)
                .cloned())
        }

        fn list(&self, query: &DeferralListQuery) -> Result<Vec<Deferral>, ApiError> {
            let deferrals = self.deferrals.lock().expect("lock is sound");
            Ok(deferrals
                .iter()
                .filter(|deferral| deferral.project_id() == query.project_id)
                .filter(|deferral| {
                    query
                        .finding_id
                        .as_ref()
                        .map(|finding_id| deferral.finding_id() == finding_id)
                        .unwrap_or(true)
                })
                .cloned()
                .collect())
        }
    }

    fn mutation() -> serde_json::Value {
        json!({ "optimistic_version": 0, "idempotency_key": "key-1" })
    }

    /// One stored Project (identity 1) every deferral test resolves.
    fn projects() -> Arc<MemoryProjectStore> {
        let projects = Arc::new(MemoryProjectStore::default());
        projects.seed(stored_project(1, "CORE", "kanban-main"));
        projects
    }

    #[test]
    fn recording_appends_an_immutable_deferral() {
        let store = Arc::new(MemoryDeferralStore::default());
        let handler = RecordDeferral {
            store: store.clone(),
            projects: projects(),
        };
        let response = handler
            .apply(
                &ParsedCommand {
                    aggregate: "deferral".to_owned(),
                    payload: json!({
                        "mutation": mutation(),
                        "project_id": 1,
                        "finding_id": "finding-1",
                        "reason": "Cosmetic only",
                    }),
                    optimistic_version: 0,
                    idempotency_key: "key-1".to_owned(),
                    fingerprint: "deferral:{}".to_owned(),
                },
                &NoopCommandEffects,
            )
            .expect("recording succeeds");

        assert_eq!(response["reason"], json!("Cosmetic only"));
        assert_eq!(response["supersedes_id"], json!(null));
        let listed = ListDeferrals {
            store,
            projects: projects(),
        }
        .handle(&json!({ "project_id": 1 }))
        .expect("list serves");
        assert_eq!(listed["deferrals"].as_array().expect("array").len(), 1);
    }

    #[test]
    fn no_edit_command_exists_in_the_catalog() {
        let names: Vec<_> = crate::catalog::exposed_operations()
            .iter()
            .map(|operation| operation.name)
            .collect();
        assert!(names.contains(&"deferral.record"));
        assert!(names.contains(&"deferral.supersede"));
        assert!(names.contains(&"deferral.list"));
        assert!(
            !names
                .iter()
                .any(|name| name.contains("deferral.edit") || name.contains("deferral.update")),
            "no edit path exists: {names:?}"
        );
    }

    #[test]
    fn superseding_creates_a_new_record_referencing_the_original() {
        let store = Arc::new(MemoryDeferralStore::default());
        let record = RecordDeferral {
            store: store.clone(),
            projects: projects(),
        };
        let supersede = SupersedeDeferral {
            store: store.clone(),
            projects: projects(),
        };
        let original = record
            .apply(
                &ParsedCommand {
                    aggregate: "deferral".to_owned(),
                    payload: json!({
                        "mutation": mutation(),
                        "project_id": 1,
                        "finding_id": "finding-1",
                        "reason": "Cosmetic only",
                    }),
                    optimistic_version: 0,
                    idempotency_key: "key-1".to_owned(),
                    fingerprint: "deferral:{}".to_owned(),
                },
                &NoopCommandEffects,
            )
            .expect("original lands");
        let replacement = supersede
            .apply(
                &ParsedCommand {
                    aggregate: "deferral".to_owned(),
                    payload: json!({
                        "mutation": mutation(),
                        "project_id": 1,
                        "deferral_id": original["id"],
                        "reason": "Accepted risk",
                    }),
                    optimistic_version: 0,
                    idempotency_key: "key-2".to_owned(),
                    fingerprint: "deferral:{}".to_owned(),
                },
                &NoopCommandEffects,
            )
            .expect("supersession lands");

        assert_eq!(replacement["supersedes_id"], original["id"]);
        let listed = ListDeferrals {
            store,
            projects: projects(),
        }
        .handle(&json!({ "project_id": 1 }))
        .expect("list serves");
        let deferrals = listed["deferrals"].as_array().expect("array");
        assert_eq!(deferrals.len(), 2);
        assert_eq!(deferrals[0]["reason"], json!("Cosmetic only"));
        assert_eq!(deferrals[1]["reason"], json!("Accepted risk"));
    }

    #[test]
    fn superseding_an_unknown_deferral_is_not_found() {
        let store = Arc::new(MemoryDeferralStore::default());
        let handler = SupersedeDeferral {
            store,
            projects: projects(),
        };
        let error = handler
            .apply(
                &ParsedCommand {
                    aggregate: "deferral".to_owned(),
                    payload: json!({
                        "mutation": mutation(),
                        "project_id": 1,
                        "deferral_id": 9,
                        "reason": "Accepted risk",
                    }),
                    optimistic_version: 0,
                    idempotency_key: "key-1".to_owned(),
                    fingerprint: "deferral:{}".to_owned(),
                },
                &NoopCommandEffects,
            )
            .expect_err("unknown deferrals are refused");

        assert_eq!(error.code, ErrorCode::NotFound);
    }
}
