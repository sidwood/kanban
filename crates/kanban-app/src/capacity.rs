//! Capacity settings commands and queries: the global defaults that
//! constrain active runs by harness, model family, and usage pool
//! across every Project (DR-EP-06), and the stricter caps and
//! maximum active Lane count one Project may impose on top
//! (DR-EP-07, KAN-S7-US3). The pure evaluation in `kanban-domain`
//! answers whether a run fits; these operations only edit the
//! limits it reads. A Project cap is refused when it would relax the
//! global default on the same dimension, and a cap a request omits
//! is a cap the Project does not set.

use std::sync::Arc;

use kanban_domain::{GlobalCapacity, ProjectCapacity, ProjectId};
use kanban_dto::{
    ApiError, CapacityDefaultsGetQuery, CapacityDefaultsGetResponse, CapacityDefaultsUpdateRequest,
    CapacityGlobalDefaults, CapacityProjectCaps, CapacitySettingsGetQuery,
    CapacitySettingsGetResponse, CapacitySettingsUpdateRequest,
};
use serde_json::Value;

use crate::dispatch::{Core, QueryHandler, RegistrationError};
use crate::mutation::{CommandHandler, ParsedCommand, parse_payload};
use crate::project::ProjectStore;

/// The storage port the capacity settings operations call through.
/// Implementations land the row change unchanged; versions guard
/// updates. A Project that never set caps holds no row: reads
/// answer unset caps at version 1, and the first update inserts.
pub trait CapacityStore: Send + Sync {
    /// The global capacity defaults.
    fn global_defaults(&self) -> Result<CapacityGlobalDefaults, ApiError>;
    /// Replace the global capacity defaults.
    fn update_global_defaults(
        &self,
        request: &CapacityDefaultsUpdateRequest,
    ) -> Result<CapacityGlobalDefaults, ApiError>;
    /// One Project's caps, unset when it imposes none.
    fn project_caps(&self, project_id: u64) -> Result<CapacityProjectCaps, ApiError>;
    /// Replace one Project's caps.
    fn update_project_caps(
        &self,
        request: &CapacitySettingsUpdateRequest,
    ) -> Result<CapacityProjectCaps, ApiError>;
}

impl Core {
    /// Register the capacity settings operations against `store`,
    /// resolving Projects through `projects`.
    pub fn register_capacity(
        &mut self,
        store: Arc<dyn CapacityStore>,
        projects: Arc<dyn ProjectStore>,
    ) -> Result<(), RegistrationError> {
        self.register_query(
            "capacity.defaults.get",
            Arc::new(GetCapacityDefaults {
                store: store.clone(),
            }),
        )?;
        self.register_command(
            "capacity.defaults.update",
            Arc::new(UpdateCapacityDefaults {
                store: store.clone(),
            }),
        )?;
        self.register_query(
            "capacity.settings.get",
            Arc::new(GetCapacitySettings {
                store: store.clone(),
                projects: projects.clone(),
            }),
        )?;
        self.register_command(
            "capacity.settings.update",
            Arc::new(UpdateCapacitySettings { store, projects }),
        )?;
        Ok(())
    }
}

/// Serves `capacity.defaults.get`.
struct GetCapacityDefaults {
    store: Arc<dyn CapacityStore>,
}

impl QueryHandler for GetCapacityDefaults {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        parse_payload::<CapacityDefaultsGetQuery>(payload)?;
        let response = CapacityDefaultsGetResponse {
            defaults: self.store.global_defaults()?,
        };
        serde_json::to_value(response).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

/// Serves `capacity.defaults.update`.
struct UpdateCapacityDefaults {
    store: Arc<dyn CapacityStore>,
}

impl CommandHandler for UpdateCapacityDefaults {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<CapacityDefaultsUpdateRequest>(payload)?;
        ParsedCommand::lift("capacity", payload)
    }

    fn current_version(&self, _command: &ParsedCommand) -> Result<u64, ApiError> {
        Ok(self.store.global_defaults()?.version)
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        _effects: &dyn crate::CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: CapacityDefaultsUpdateRequest = parse_payload(&command.payload)?;
        GlobalCapacity::new(
            request.max_active_per_harness,
            request.max_active_per_model,
            request.max_active_per_usage_pool,
        )
        .map_err(refuse)?;
        let defaults = self.store.update_global_defaults(&request)?;
        serde_json::to_value(defaults).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

/// Serves `capacity.settings.get`.
struct GetCapacitySettings {
    store: Arc<dyn CapacityStore>,
    projects: Arc<dyn ProjectStore>,
}

impl QueryHandler for GetCapacitySettings {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        let query: CapacitySettingsGetQuery = parse_payload(payload)?;
        let project = self
            .projects
            .find(ProjectId::new(query.project_id))?
            .ok_or_else(|| ApiError::not_found(&format!("project {}", query.project_id)))?;
        let response = CapacitySettingsGetResponse {
            project_id: project.id().value(),
            caps: self.store.project_caps(project.id().value())?,
        };
        serde_json::to_value(response).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

/// Serves `capacity.settings.update`.
struct UpdateCapacitySettings {
    store: Arc<dyn CapacityStore>,
    projects: Arc<dyn ProjectStore>,
}

impl CommandHandler for UpdateCapacitySettings {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<CapacitySettingsUpdateRequest>(payload)?;
        ParsedCommand::lift("capacity", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: CapacitySettingsUpdateRequest = parse_payload(&command.payload)?;
        self.project(&request.project_id)?;
        Ok(self.store.project_caps(request.project_id)?.version)
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        _effects: &dyn crate::CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: CapacitySettingsUpdateRequest = parse_payload(&command.payload)?;
        self.project(&request.project_id)?;
        // The caps are judged against the global defaults they can
        // only tighten, never relax.
        let global = self.store.global_defaults()?;
        ProjectCapacity::new(
            &GlobalCapacity::restore(
                global.max_active_per_harness,
                global.max_active_per_model,
                global.max_active_per_usage_pool,
            ),
            request.max_active_per_harness,
            request.max_active_per_model,
            request.max_active_per_usage_pool,
            request.max_active_lanes,
        )
        .map_err(refuse)?;
        let caps = self.store.update_project_caps(&request)?;
        serde_json::to_value(caps).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

impl UpdateCapacitySettings {
    /// The Project the command addresses, or the stable not-found
    /// refusal.
    fn project(&self, project_id: &u64) -> Result<(), ApiError> {
        self.projects
            .find(ProjectId::new(*project_id))?
            .ok_or_else(|| ApiError::not_found(&format!("project {project_id}")))?;
        Ok(())
    }
}

/// Report a refused domain rule as the stable invalid-request code.
fn refuse(error: kanban_domain::CapacityError) -> ApiError {
    ApiError::invalid_request(&error.to_string())
}

#[cfg(test)]
mod project_caps {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use serde_json::json;

    use kanban_dto::{ApiError, ErrorCode, MutationContext};

    use super::CapacityStore;
    use crate::dispatch::Core;
    use crate::events::NoopEventSink;
    use crate::mutation::MemoryIdempotencyStore;
    use crate::plan::testing::{MemoryProjects, active_project};

    /// An in-memory capacity store: global defaults seeded like the
    /// migration, per-Project caps materialised on first write.
    struct MemoryCapacity {
        defaults: Mutex<CapacityDefaults>,
        caps: Mutex<HashMap<u64, StoredCaps>>,
    }

    type CapacityDefaults = kanban_dto::CapacityGlobalDefaults;
    type StoredCaps = kanban_dto::CapacityProjectCaps;

    impl MemoryCapacity {
        fn seeded() -> Self {
            Self {
                defaults: Mutex::new(CapacityDefaults {
                    max_active_per_harness: 2,
                    max_active_per_model: 2,
                    max_active_per_usage_pool: 4,
                    version: 1,
                }),
                caps: Mutex::new(HashMap::new()),
            }
        }
    }

    impl CapacityStore for MemoryCapacity {
        fn global_defaults(&self) -> Result<CapacityDefaults, ApiError> {
            Ok(*self.defaults.lock().expect("the defaults lock is sound"))
        }

        fn update_global_defaults(
            &self,
            request: &kanban_dto::CapacityDefaultsUpdateRequest,
        ) -> Result<CapacityDefaults, ApiError> {
            let mut defaults = self.defaults.lock().expect("the defaults lock is sound");
            if request.mutation.optimistic_version != defaults.version {
                return Err(ApiError::stale_version(
                    request.mutation.optimistic_version,
                    defaults.version,
                ));
            }
            *defaults = CapacityDefaults {
                max_active_per_harness: request.max_active_per_harness,
                max_active_per_model: request.max_active_per_model,
                max_active_per_usage_pool: request.max_active_per_usage_pool,
                version: defaults.version + 1,
            };
            Ok(*defaults)
        }

        fn project_caps(&self, project_id: u64) -> Result<StoredCaps, ApiError> {
            Ok(self
                .caps
                .lock()
                .expect("the caps lock is sound")
                .get(&project_id)
                .copied()
                .unwrap_or(StoredCaps {
                    max_active_per_harness: None,
                    max_active_per_model: None,
                    max_active_per_usage_pool: None,
                    max_active_lanes: None,
                    version: 1,
                }))
        }

        fn update_project_caps(
            &self,
            request: &kanban_dto::CapacitySettingsUpdateRequest,
        ) -> Result<StoredCaps, ApiError> {
            // Read before taking the write lock: project_caps locks
            // the same Mutex, and a std Mutex is not reentrant.
            let current = self.project_caps(request.project_id)?;
            let mut caps = self.caps.lock().expect("the caps lock is sound");
            if request.mutation.optimistic_version != current.version {
                return Err(ApiError::stale_version(
                    request.mutation.optimistic_version,
                    current.version,
                ));
            }
            let updated = StoredCaps {
                max_active_per_harness: request.max_active_per_harness,
                max_active_per_model: request.max_active_per_model,
                max_active_per_usage_pool: request.max_active_per_usage_pool,
                max_active_lanes: request.max_active_lanes,
                version: current.version + 1,
            };
            caps.insert(request.project_id, updated);
            Ok(updated)
        }
    }

    /// A core with the capacity operations wired over one active
    /// Project, plus the stores for assertions.
    fn harness() -> (Core, Arc<MemoryCapacity>, Arc<MemoryProjects>) {
        let projects = Arc::new(MemoryProjects::default());
        projects.seed(active_project(
            1,
            "CORE",
            kanban_domain::ProjectCounters::zeroed(),
        ));
        let capacity = Arc::new(MemoryCapacity::seeded());
        let mut core = Core::new(
            crate::catalog::exposed_operations(),
            Arc::new(MemoryIdempotencyStore::new()),
            Arc::new(NoopEventSink),
        );
        core.register_capacity(capacity.clone(), projects.clone())
            .expect("the capacity operations register");
        (core, capacity, projects)
    }

    fn mutation(version: u64, key: &str) -> MutationContext {
        MutationContext {
            optimistic_version: version,
            idempotency_key: key.to_owned(),
        }
    }

    #[test]
    fn defaults_get_serves_the_global_limits() {
        let (core, _, _) = harness();

        let response = core
            .query("capacity.defaults.get", &json!({}))
            .expect("the defaults serve");

        assert_eq!(
            response,
            json!({
                "defaults": {
                    "max_active_per_harness": 2,
                    "max_active_per_model": 2,
                    "max_active_per_usage_pool": 4,
                    "version": 1,
                }
            })
        );
    }

    #[test]
    fn defaults_update_replaces_the_limits() {
        let (core, store, _) = harness();

        let response = core
            .command(
                "capacity.defaults.update",
                &json!({
                    "mutation": mutation(1, "key-defaults"),
                    "max_active_per_harness": 3,
                    "max_active_per_model": 1,
                    "max_active_per_usage_pool": 5,
                }),
            )
            .expect("the update lands");

        assert_eq!(response["max_active_per_model"], json!(1));
        assert_eq!(response["version"], json!(2));
        assert_eq!(
            store.global_defaults().expect("the store serves").version,
            2
        );
    }

    #[test]
    fn a_zero_limit_is_refused_without_recording() {
        let (core, store, _) = harness();

        let error = core
            .command(
                "capacity.defaults.update",
                &json!({
                    "mutation": mutation(1, "key-zero"),
                    "max_active_per_harness": 0,
                    "max_active_per_model": 2,
                    "max_active_per_usage_pool": 4,
                }),
            )
            .expect_err("a zero limit is refused");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "a harness capacity limit must be greater than zero"
        );
        assert_eq!(
            store.global_defaults().expect("the store serves").version,
            1,
            "the refusal changed nothing"
        );
    }

    #[test]
    fn settings_get_serves_unset_caps_for_a_fresh_project() {
        let (core, _, _) = harness();

        let response = core
            .query("capacity.settings.get", &json!({ "project_id": 1 }))
            .expect("the caps serve");

        assert_eq!(
            response,
            json!({
                "project_id": 1,
                "caps": {
                    "max_active_per_harness": null,
                    "max_active_per_model": null,
                    "max_active_per_usage_pool": null,
                    "max_active_lanes": null,
                    "version": 1,
                }
            }),
            "a Project that imposes nothing shows every cap null"
        );
    }

    #[test]
    fn settings_for_an_unknown_project_is_not_found() {
        let (core, _, _) = harness();

        let error = core
            .query("capacity.settings.get", &json!({ "project_id": 9 }))
            .expect_err("the unknown Project is refused");
        assert_eq!(error.code, ErrorCode::NotFound);

        let error = core
            .command(
                "capacity.settings.update",
                &json!({
                    "mutation": mutation(1, "key-unknown"),
                    "project_id": 9,
                    "max_active_lanes": 2,
                }),
            )
            .expect_err("the unknown Project is refused");
        assert_eq!(error.code, ErrorCode::NotFound);
    }

    #[test]
    fn stricter_caps_land_and_clear_by_omission() {
        let (core, _, _) = harness();

        let response = core
            .command(
                "capacity.settings.update",
                &json!({
                    "mutation": mutation(1, "key-caps"),
                    "project_id": 1,
                    "max_active_per_harness": 2,
                    "max_active_lanes": 3,
                }),
            )
            .expect("the stricter caps land");
        assert_eq!(response["max_active_per_harness"], json!(2));
        assert_eq!(response["max_active_lanes"], json!(3));
        assert_eq!(response["version"], json!(2));

        // The next update replaces the caps wholesale, so omitting
        // the harness cap clears it back to the global default.
        let response = core
            .command(
                "capacity.settings.update",
                &json!({
                    "mutation": mutation(2, "key-clear"),
                    "project_id": 1,
                    "max_active_lanes": 3,
                }),
            )
            .expect("the clearing update lands");
        assert_eq!(response["max_active_per_harness"], json!(null));
        assert_eq!(response["max_active_lanes"], json!(3));
    }

    #[test]
    fn a_cap_above_the_global_default_is_refused() {
        let (core, _, _) = harness();

        let error = core
            .command(
                "capacity.settings.update",
                &json!({
                    "mutation": mutation(1, "key-relax"),
                    "project_id": 1,
                    "max_active_per_harness": 99,
                }),
            )
            .expect_err("a relaxing cap is refused");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "a Project harness limit of 99 would relax the global 2"
        );
        let read = core
            .query("capacity.settings.get", &json!({ "project_id": 1 }))
            .expect("the caps serve");
        assert_eq!(
            read["caps"]["max_active_per_harness"],
            json!(null),
            "the refusal changed nothing"
        );
    }

    #[test]
    fn a_relaxed_global_default_still_refuses_above_it() {
        let (core, _, _) = harness();

        // The global harness default is 2, so a cap of 3 is refused
        // even after other dimensions carry values.
        let error = core
            .command(
                "capacity.settings.update",
                &json!({
                    "mutation": mutation(1, "key-mixed"),
                    "project_id": 1,
                    "max_active_per_harness": 1,
                    "max_active_per_model": 3,
                }),
            )
            .expect_err("a model cap above the global default is refused");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "a Project model family limit of 3 would relax the global 2"
        );
    }

    #[test]
    fn a_stale_version_is_rejected_with_the_current_one() {
        let (core, _, _) = harness();

        let error = core
            .command(
                "capacity.settings.update",
                &json!({
                    "mutation": mutation(4, "key-stale"),
                    "project_id": 1,
                    "max_active_lanes": 2,
                }),
            )
            .expect_err("the stale version is rejected");

        assert_eq!(error.code, ErrorCode::StaleVersion);
        assert_eq!(error.current_version, Some(1));
    }

    #[test]
    fn every_operation_rejects_unknown_fields() {
        let (core, _, _) = harness();

        let mut request = json!({
            "mutation": mutation(1, "key-surprise"),
            "max_active_per_harness": 1,
            "max_active_per_model": 1,
            "max_active_per_usage_pool": 1,
        });
        request["surprise"] = json!(true);
        let error = core
            .command("capacity.defaults.update", &request)
            .expect_err("unknown fields are rejected");
        assert_eq!(error.code, ErrorCode::UnknownField);
        assert_eq!(error.message, "unknown field `surprise`");
    }
}
