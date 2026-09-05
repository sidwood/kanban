//! Herdr observation settings and connection diagnostics (KAN-S8).

use std::sync::Arc;

use kanban_domain::Project;
use kanban_domain::ProjectId;
use kanban_dto::{
    ApiError, HerdrConnectionDiagnostics, HerdrDefaultsGetQuery, HerdrDefaultsGetResponse,
    HerdrDefaultsUpdateRequest, HerdrGlobalDefaults, HerdrProjectSettings, HerdrSettingsGetQuery,
    HerdrSettingsGetResponse, HerdrSettingsUpdateRequest,
};
use serde_json::Value;

use crate::dispatch::{Core, QueryHandler, RegistrationError};
use crate::mutation::{CommandHandler, ParsedCommand, parse_payload};
use crate::project::ProjectStore;

/// The storage port for Herdr settings.
pub trait HerdrSettingsStore: Send + Sync {
    /// The global defaults every new Project inherits.
    fn global_defaults(&self) -> Result<HerdrGlobalDefaults, ApiError>;
    /// Replace the global defaults.
    fn update_global_defaults(
        &self,
        request: &HerdrDefaultsUpdateRequest,
    ) -> Result<HerdrGlobalDefaults, ApiError>;
    /// One Project's Herdr settings.
    fn project_settings(&self, project_id: u64) -> Result<HerdrProjectSettings, ApiError>;
    /// Replace one Project's Herdr settings.
    fn update_project_settings(
        &self,
        request: &HerdrSettingsUpdateRequest,
    ) -> Result<HerdrProjectSettings, ApiError>;
    /// Seed settings for a freshly registered Project.
    fn seed_project_settings(&self, project_id: u64) -> Result<(), ApiError>;
}

/// Starts or continues Herdr observation for one Project.
pub trait HerdrProjectObserver: Send + Sync {
    /// Observe one active Project's Herdr session.
    fn observe(&self, project: &Project);
    /// Stop observing one Project's session, releasing its socket
    /// and database references.
    fn stop_observing(&self, project_id: u64);
}

/// A test double that records no observation.
#[derive(Debug, Default)]
pub struct NoopHerdrProjectObserver;

impl HerdrProjectObserver for NoopHerdrProjectObserver {
    fn observe(&self, _project: &Project) {}

    fn stop_observing(&self, _project_id: u64) {}
}

/// Live connection diagnostics maintained by the service observer.
pub trait HerdrDiagnostics: Send + Sync {
    /// The current diagnostics for one Project's Herdr binding: the
    /// session the Project selected, if any, the product workspace,
    /// and the target Herdr workspace, resolved together so identity
    /// never escapes the effective session (DR-HB-19).
    fn for_project(
        &self,
        project_id: u64,
        session: Option<&str>,
        product_workspace: &str,
        herdr_workspace: &str,
    ) -> HerdrConnectionDiagnostics;
}

impl Core {
    /// Register Herdr settings and diagnostics queries.
    pub fn register_herdr(
        &mut self,
        settings: Arc<dyn HerdrSettingsStore>,
        diagnostics: Arc<dyn HerdrDiagnostics>,
        projects: Arc<dyn ProjectStore>,
    ) -> Result<(), RegistrationError> {
        self.register_query(
            "herdr.settings.get",
            Arc::new(GetHerdrSettings {
                settings: settings.clone(),
                diagnostics: diagnostics.clone(),
                projects: projects.clone(),
            }),
        )?;
        self.register_command(
            "herdr.settings.update",
            Arc::new(UpdateHerdrSettings {
                store: settings.clone(),
            }),
        )?;
        self.register_query(
            "herdr.defaults.get",
            Arc::new(GetHerdrDefaults {
                store: settings.clone(),
            }),
        )?;
        self.register_command(
            "herdr.defaults.update",
            Arc::new(UpdateHerdrDefaults { store: settings }),
        )?;
        Ok(())
    }
}

struct GetHerdrSettings {
    settings: Arc<dyn HerdrSettingsStore>,
    diagnostics: Arc<dyn HerdrDiagnostics>,
    projects: Arc<dyn ProjectStore>,
}

impl QueryHandler for GetHerdrSettings {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        let query: HerdrSettingsGetQuery = parse_payload(payload)?;
        let project = self
            .projects
            .find(ProjectId::new(query.project_id))?
            .ok_or_else(|| ApiError::not_found(&format!("project {}", query.project_id)))?;
        let registration = project.registration();
        let settings = self.settings.project_settings(query.project_id)?;
        let diagnostics = self.diagnostics.for_project(
            query.project_id,
            registration.herdr_session(),
            registration.seed_workspace(),
            registration.herdr_workspace(),
        );
        let response = HerdrSettingsGetResponse {
            project_id: query.project_id,
            settings,
            diagnostics,
        };
        serde_json::to_value(response).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

struct UpdateHerdrSettings {
    store: Arc<dyn HerdrSettingsStore>,
}

impl CommandHandler for UpdateHerdrSettings {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<HerdrSettingsUpdateRequest>(payload)?;
        ParsedCommand::lift("herdr", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: HerdrSettingsUpdateRequest = parse_payload(&command.payload)?;
        Ok(self.store.project_settings(request.project_id)?.version)
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        _events: &dyn crate::EventSink,
    ) -> Result<Value, ApiError> {
        let request: HerdrSettingsUpdateRequest = parse_payload(&command.payload)?;
        validate_settings(&request)?;
        let settings = self.store.update_project_settings(&request)?;
        serde_json::to_value(settings).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

struct GetHerdrDefaults {
    store: Arc<dyn HerdrSettingsStore>,
}

impl QueryHandler for GetHerdrDefaults {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        let _: HerdrDefaultsGetQuery = parse_payload(payload)?;
        let defaults = self.store.global_defaults()?;
        let response = HerdrDefaultsGetResponse { defaults };
        serde_json::to_value(response).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

struct UpdateHerdrDefaults {
    store: Arc<dyn HerdrSettingsStore>,
}

impl CommandHandler for UpdateHerdrDefaults {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<HerdrDefaultsUpdateRequest>(payload)?;
        ParsedCommand::lift("herdr", payload)
    }

    fn current_version(&self, _command: &ParsedCommand) -> Result<u64, ApiError> {
        Ok(self.store.global_defaults()?.version)
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        _events: &dyn crate::EventSink,
    ) -> Result<Value, ApiError> {
        let request: HerdrDefaultsUpdateRequest = parse_payload(&command.payload)?;
        if request.reconciliation_interval_secs == 0 {
            return Err(ApiError::invalid_request(
                "reconciliation interval must be greater than zero",
            ));
        }
        if request.stall_deadline_secs == 0 || request.missing_result_deadline_secs == 0 {
            return Err(ApiError::invalid_request(
                "deadlines must be greater than zero",
            ));
        }
        let defaults = self.store.update_global_defaults(&request)?;
        serde_json::to_value(defaults).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

fn validate_settings(request: &HerdrSettingsUpdateRequest) -> Result<(), ApiError> {
    if request.reconciliation_interval_secs == 0 {
        return Err(ApiError::invalid_request(
            "reconciliation interval must be greater than zero",
        ));
    }
    if request.polling_fallback_interval_secs == 0 {
        return Err(ApiError::invalid_request(
            "polling fallback interval must be greater than zero",
        ));
    }
    if request.stall_deadline_secs == 0 || request.missing_result_deadline_secs == 0 {
        return Err(ApiError::invalid_request(
            "deadlines must be greater than zero",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use serde_json::json;

    use super::{
        GetHerdrDefaults, GetHerdrSettings, HerdrDiagnostics, HerdrSettingsStore,
        UpdateHerdrDefaults, UpdateHerdrSettings,
    };
    use crate::dispatch::QueryHandler;
    use crate::mutation::CommandHandler;
    use crate::project::ProjectStore;
    use crate::{NoopEventSink, TimelineEnvelope};
    use kanban_domain::{Project, ProjectId, ProjectRegistration};
    use kanban_dto::{
        ApiError, HerdrConnectionDiagnostics, HerdrDefaultsGetQuery, HerdrDefaultsUpdateRequest,
        HerdrGlobalDefaults, HerdrProjectSettings, HerdrSettingsUpdateRequest, TimelineEventKind,
    };

    struct MemorySettings {
        defaults: Mutex<HerdrGlobalDefaults>,
        projects: Mutex<HashMap<u64, HerdrProjectSettings>>,
    }

    impl MemorySettings {
        fn new() -> Self {
            Self {
                defaults: Mutex::new(HerdrGlobalDefaults {
                    reconciliation_interval_secs: 300,
                    stall_deadline_secs: 3600,
                    missing_result_deadline_secs: 7200,
                    version: 1,
                }),
                projects: Mutex::new(HashMap::new()),
            }
        }
    }

    impl HerdrSettingsStore for MemorySettings {
        fn global_defaults(&self) -> Result<HerdrGlobalDefaults, ApiError> {
            Ok(self.defaults.lock().unwrap().clone())
        }

        fn update_global_defaults(
            &self,
            request: &HerdrDefaultsUpdateRequest,
        ) -> Result<HerdrGlobalDefaults, ApiError> {
            let mut defaults = self.defaults.lock().unwrap();
            if request.mutation.optimistic_version != defaults.version {
                return Err(ApiError::stale_version(
                    request.mutation.optimistic_version,
                    defaults.version,
                ));
            }
            *defaults = HerdrGlobalDefaults {
                reconciliation_interval_secs: request.reconciliation_interval_secs,
                stall_deadline_secs: request.stall_deadline_secs,
                missing_result_deadline_secs: request.missing_result_deadline_secs,
                version: defaults.version + 1,
            };
            Ok(defaults.clone())
        }

        fn project_settings(&self, project_id: u64) -> Result<HerdrProjectSettings, ApiError> {
            self.projects
                .lock()
                .unwrap()
                .get(&project_id)
                .cloned()
                .ok_or_else(|| {
                    ApiError::not_found(&format!("herdr settings for project {project_id}"))
                })
        }

        fn update_project_settings(
            &self,
            request: &HerdrSettingsUpdateRequest,
        ) -> Result<HerdrProjectSettings, ApiError> {
            let mut projects = self.projects.lock().unwrap();
            let current = projects
                .get(&request.project_id)
                .ok_or_else(|| ApiError::not_found(&format!("project {}", request.project_id)))?;
            if request.mutation.optimistic_version != current.version {
                return Err(ApiError::stale_version(
                    request.mutation.optimistic_version,
                    current.version,
                ));
            }
            let updated = HerdrProjectSettings {
                reconciliation_interval_secs: request.reconciliation_interval_secs,
                polling_fallback_enabled: request.polling_fallback_enabled,
                polling_fallback_interval_secs: request.polling_fallback_interval_secs,
                stall_deadline_secs: request.stall_deadline_secs,
                missing_result_deadline_secs: request.missing_result_deadline_secs,
                version: current.version + 1,
            };
            projects.insert(request.project_id, updated.clone());
            Ok(updated)
        }

        fn seed_project_settings(&self, project_id: u64) -> Result<(), ApiError> {
            let defaults = self.global_defaults()?;
            self.projects.lock().unwrap().insert(
                project_id,
                HerdrProjectSettings {
                    reconciliation_interval_secs: defaults.reconciliation_interval_secs,
                    polling_fallback_enabled: false,
                    polling_fallback_interval_secs: 10,
                    stall_deadline_secs: defaults.stall_deadline_secs,
                    missing_result_deadline_secs: defaults.missing_result_deadline_secs,
                    version: 1,
                },
            );
            Ok(())
        }
    }

    struct StaticDiagnostics;

    impl HerdrDiagnostics for StaticDiagnostics {
        fn for_project(
            &self,
            _project_id: u64,
            session: Option<&str>,
            product_workspace: &str,
            herdr_workspace: &str,
        ) -> HerdrConnectionDiagnostics {
            HerdrConnectionDiagnostics {
                session_name: session.map(str::to_owned),
                product_workspace: product_workspace.to_owned(),
                herdr_workspace: herdr_workspace.to_owned(),
                connected: true,
                last_snapshot_at: Some("2026-09-05T04:46:00Z".to_owned()),
                last_error: None,
            }
        }
    }

    struct MemoryProjects(Mutex<Vec<Project>>);

    impl ProjectStore for MemoryProjects {
        fn create(
            &self,
            registration: &ProjectRegistration,
            _envelope: &dyn Fn(ProjectId) -> TimelineEnvelope,
        ) -> Result<Project, ApiError> {
            let id = ProjectId::new(self.0.lock().unwrap().len() as u64 + 1);
            let project = Project::new(id, registration.clone());
            self.0.lock().unwrap().push(project.clone());
            Ok(project)
        }

        fn find(&self, id: ProjectId) -> Result<Option<Project>, ApiError> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .iter()
                .find(|project| project.id() == id)
                .cloned())
        }

        fn save(&self, _project: &Project, _envelope: TimelineEnvelope) -> Result<(), ApiError> {
            Ok(())
        }

        fn list(&self) -> Result<Vec<Project>, ApiError> {
            Ok(self.0.lock().unwrap().clone())
        }
    }

    fn registered_project() -> (Arc<MemorySettings>, Arc<MemoryProjects>, u64) {
        let settings = Arc::new(MemorySettings::new());
        let projects = Arc::new(MemoryProjects(Mutex::new(Vec::new())));
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
        .expect("the registration validates");
        let project = projects
            .create(&registration, &|_| {
                TimelineEnvelope::project(
                    "1",
                    TimelineEventKind::Transition,
                    None,
                    json!({ "action": "registered" }),
                )
                .expect("the envelope validates")
            })
            .expect("the project registers");
        settings
            .seed_project_settings(project.id().value())
            .expect("settings seed");
        (settings, projects, project.id().value())
    }

    #[test]
    fn herdr_settings_get_returns_settings_and_diagnostics() {
        let (settings, projects, project_id) = registered_project();
        let handler = GetHerdrSettings {
            settings,
            diagnostics: Arc::new(StaticDiagnostics),
            projects,
        };
        let response = handler
            .handle(&json!({ "project_id": project_id }))
            .expect("settings and diagnostics are served");
        assert_eq!(
            response["settings"]["reconciliation_interval_secs"],
            json!(300)
        );
        assert_eq!(response["diagnostics"]["connected"], json!(true));
        assert_eq!(
            response["diagnostics"]["session_name"],
            json!("kanban-main")
        );
        assert_eq!(
            response["diagnostics"]["herdr_workspace"],
            json!("kanban.seed"),
            "diagnostics carry the target workspace identity"
        );
    }

    #[test]
    fn herdr_settings_get_reports_a_default_session_without_a_name() {
        let (settings, projects, project_id) = registered_project();
        let mut projects_store = projects.0.lock().unwrap();
        let sessionless = ProjectRegistration::new(
            "WAVE",
            "Wave pool",
            "/repositories/kanban",
            "/workspaces/wave.seed",
            "main",
            "wave.seed",
            None,
            None,
        )
        .expect("the registration validates");
        *projects_store.first_mut().unwrap() =
            Project::new(ProjectId::new(project_id), sessionless);
        drop(projects_store);
        let handler = GetHerdrSettings {
            settings,
            diagnostics: Arc::new(StaticDiagnostics),
            projects,
        };

        let response = handler
            .handle(&json!({ "project_id": project_id }))
            .expect("settings and diagnostics are served");

        assert!(
            response["diagnostics"]["session_name"].is_null(),
            "an unnamed session reports no name, not an empty one"
        );
        assert_eq!(
            response["diagnostics"]["herdr_workspace"],
            json!("wave.seed")
        );
    }

    #[test]
    fn herdr_settings_update_changes_project_settings() {
        let (settings, _, project_id) = registered_project();
        let handler = UpdateHerdrSettings {
            store: settings.clone(),
        };
        let payload = json!({
            "mutation": { "optimistic_version": 1, "idempotency_key": "herdr-1" },
            "project_id": project_id,
            "reconciliation_interval_secs": 600,
            "polling_fallback_enabled": true,
            "polling_fallback_interval_secs": 10,
            "stall_deadline_secs": 1800,
            "missing_result_deadline_secs": 3600
        });
        let command = handler.parse(&payload).expect("the command parses");
        let updated = handler
            .apply(&command, &NoopEventSink)
            .expect("the update lands");
        assert_eq!(updated["polling_fallback_enabled"], json!(true));
        assert_eq!(updated["version"], json!(2));
    }

    #[test]
    fn herdr_defaults_update_changes_global_defaults() {
        let settings = Arc::new(MemorySettings::new());
        let handler = UpdateHerdrDefaults {
            store: settings.clone(),
        };
        let payload = json!({
            "mutation": { "optimistic_version": 1, "idempotency_key": "defaults-1" },
            "reconciliation_interval_secs": 900,
            "stall_deadline_secs": 7200,
            "missing_result_deadline_secs": 14400
        });
        let command = handler.parse(&payload).expect("the command parses");
        let updated = handler
            .apply(&command, &NoopEventSink)
            .expect("defaults update");
        assert_eq!(updated["reconciliation_interval_secs"], json!(900));
    }

    #[test]
    fn herdr_defaults_get_returns_global_defaults() {
        let settings = Arc::new(MemorySettings::new());
        let handler = GetHerdrDefaults { store: settings };
        let response = handler
            .handle(&serde_json::to_value(HerdrDefaultsGetQuery {}).expect("query encodes"))
            .expect("defaults are served");
        assert_eq!(
            response["defaults"]["reconciliation_interval_secs"],
            json!(300)
        );
    }
}
