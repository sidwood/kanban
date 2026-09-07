//! Component health: the one `health.get` answer reporting service,
//! database, scheduler, MCP, Herdr, and Workspace state (KAN-S13-US5,
//! DR-RB-12). Every probe reads state the components already hold —
//! no observation path exists only for health.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use kanban_app::{
    EXPOSED_MCP_TOOL_NAMES, HerdrDiagnostics, ProjectStore, QueryHandler, WorkspaceStore,
};
use kanban_domain::WorkspaceHealth;
use kanban_dto::{
    ApiError, DatabaseHealth, HealthQuery, HealthResponse, HerdrHealth, HerdrSessionHealth,
    McpHealth, SchedulerHealth, ServiceHealth, WorkspaceHealthCounts, WorkspacesHealth,
};
use kanban_storage::Database;
use serde_json::Value;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::backup_scheduler::load_scheduler_state;

/// Serves `health.get` from the running core's own components: the
/// answering process is the service, the open database is the
/// database, the persisted backup state is the scheduler, the
/// catalogue is MCP, the observer's live diagnostics are Herdr, and
/// the registered records are the Workspaces. The diagnostic bundle
/// exports the same answer this handler serves.
pub struct ComponentHealthHandler {
    /// When this handler was wired — the moment the service's state
    /// came into being for this run.
    started_at: String,
    database: Arc<Database>,
    data_dir: PathBuf,
    herdr: Arc<dyn HerdrDiagnostics>,
    projects: Arc<dyn ProjectStore>,
    workspaces: Arc<dyn WorkspaceStore>,
}

impl ComponentHealthHandler {
    /// Wire the probes a running core already holds. The start time
    /// is captured here because wiring is when the service's state
    /// began.
    pub fn new(
        database: Arc<Database>,
        data_dir: PathBuf,
        herdr: Arc<dyn HerdrDiagnostics>,
        projects: Arc<dyn ProjectStore>,
        workspaces: Arc<dyn WorkspaceStore>,
    ) -> Self {
        Self {
            started_at: wire_time(SystemTime::now()),
            database,
            data_dir,
            herdr,
            projects,
            workspaces,
        }
    }

    /// The current component report. A probe that cannot read its
    /// component refuses the whole answer: health that guesses is
    /// worse than health that fails loudly.
    pub fn current(&self) -> Result<HealthResponse, ApiError> {
        let sessions = self.herdr_sessions()?;
        let by_health = self.workspace_census()?;
        Ok(HealthResponse {
            connected: true,
            service_version: env!("CARGO_PKG_VERSION").to_owned(),
            service: ServiceHealth {
                started_at: self.started_at.clone(),
            },
            database: DatabaseHealth {
                journal_mode: self.database.journal_mode().map_err(internal)?,
                schema_version: self.database.schema_version().map_err(internal)?,
                last_change_at: self.database.last_change_at().map_err(internal)?,
            },
            scheduler: SchedulerHealth {
                last_backup_success_at: load_scheduler_state(&self.data_dir).map(wire_time),
            },
            mcp: McpHealth {
                exposed_tools: EXPOSED_MCP_TOOL_NAMES.len() as u32,
            },
            herdr: HerdrHealth { sessions },
            workspaces: WorkspacesHealth {
                by_health,
                last_change_at: self.database.last_workspace_change_at().map_err(internal)?,
            },
        })
    }

    /// One session entry per observed Project — the active Projects,
    /// whose diagnostics the observer maintains — in identity order.
    fn herdr_sessions(&self) -> Result<Vec<HerdrSessionHealth>, ApiError> {
        let mut projects: Vec<_> = self.projects.list()?;
        projects.sort_by_key(|project| project.id().value());
        let mut sessions = Vec::new();
        for project in &projects {
            if project.is_archived() {
                continue;
            }
            let registration = project.registration();
            let diagnostics = self.herdr.for_project(
                project.id().value(),
                registration.herdr_session(),
                registration.seed_workspace(),
                registration.herdr_workspace(),
            );
            sessions.push(HerdrSessionHealth {
                project_id: project.id().value(),
                diagnostics,
            });
        }
        Ok(sessions)
    }

    /// The Workspace health census across every registered Project.
    fn workspace_census(&self) -> Result<WorkspaceHealthCounts, ApiError> {
        let mut counts = WorkspaceHealthCounts::default();
        for project in self.projects.list()? {
            for workspace in self.workspaces.list_for_project(project.id())? {
                match workspace.health() {
                    WorkspaceHealth::Available => counts.available += 1,
                    WorkspaceHealth::Assigned => counts.assigned += 1,
                    WorkspaceHealth::Dirty => counts.dirty += 1,
                    WorkspaceHealth::Missing => counts.missing += 1,
                    WorkspaceHealth::Retired => counts.retired += 1,
                    WorkspaceHealth::Unobserved => counts.unobserved += 1,
                }
            }
        }
        Ok(counts)
    }
}

impl QueryHandler for ComponentHealthHandler {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        kanban_app::parse_payload::<HealthQuery>(payload)?;
        let response = self.current()?;
        serde_json::to_value(response).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

/// Shapes a storage refusal for the health answer.
fn internal(error: kanban_storage::StorageError) -> ApiError {
    ApiError::internal(&error.to_string())
}

/// Renders a system time in the wire shape every recorded time uses.
fn wire_time(when: SystemTime) -> String {
    OffsetDateTime::from(when)
        .format(&Rfc3339)
        .expect("the UTC offset renders in RFC 3339")
}

#[cfg(test)]
mod tests {
    use std::thread;
    use std::time::{Duration, Instant};

    use serde_json::{Value, json};
    use tempfile::TempDir;

    use kanban_herdr::fixture::{ScriptedSession, SessionScript};

    use kanban_app::EXPOSED_MCP_TOOL_NAMES;

    use crate::serve_with_herdr_sessions;
    use crate::test_client::{Client, boot};

    /// A scratch directory standing in for a Git repository the
    /// service's own observation accepts.
    fn scratch_repository(dir: &TempDir, name: &str) -> String {
        let repository = dir.path().join(name);
        std::fs::create_dir_all(repository.join(".git"))
            .expect("the scratch repository is created");
        repository
            .canonicalize()
            .expect("the repository path canonicalises")
            .to_str()
            .expect("the path is UTF-8")
            .to_owned()
    }

    /// KAN-T62-AC1: one query reports every component's state, with
    /// the last-change times the components already record.
    #[test]
    fn health_get_reports_every_component_state_in_one_query() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_root = dir.path().join("sessions");
        let repository = scratch_repository(&dir, "kanban");
        let _fixture = ScriptedSession::bind(
            &socket_root,
            "kanban-main",
            "/workspaces/kanban.seed",
            SessionScript::default(),
        );
        let core = serve_with_herdr_sessions(dir.path(), socket_root)
            .expect("the core boots for the test");
        let mut client = Client::connect(core.socket_path());
        client.command(
            "project.register",
            json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "health-register" },
                "code": "CORE",
                "name": "Control plane",
                "repository": repository,
                "seed_workspace": "/workspaces/kanban.seed",
                "default_branch": "main",
                "herdr_session": "kanban-main",
                "herdr_workspace": "kanban.seed",
            }),
        );
        for (name, key) in [("one", "health-workspace-1"), ("two", "health-workspace-2")] {
            client.command(
                "workspace.register",
                json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": key },
                    "project_id": 1,
                    "path": format!("/workspaces/kanban.{name}"),
                }),
            );
        }

        // The Herdr snapshot lands on its own observation thread and
        // the first scheduled backup lands on its own schedule, so
        // poll the one answer until both have landed: the query is
        // the probe, never a wait beside it.
        let health = {
            let mut client = Client::connect(core.socket_path());
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                let answer = client.query("health.get");
                let herdr_settled =
                    answer["herdr"]["sessions"]
                        .as_array()
                        .is_some_and(|sessions| {
                            sessions.len() == 1
                                && sessions[0]["diagnostics"]["connected"] == json!(true)
                                && sessions[0]["diagnostics"]["last_snapshot_at"].is_string()
                        });
                let backup_settled = answer["scheduler"]["last_backup_success_at"].is_string();
                if (herdr_settled && backup_settled) || Instant::now() > deadline {
                    break answer;
                }
                assert!(
                    Instant::now() < deadline,
                    "health never reported the settled Herdr session and completed backup: {answer}"
                );
                thread::sleep(Duration::from_millis(10));
            }
        };

        // The service: answering is the connection proof, and the
        // start is the moment this core's state came into being.
        assert_eq!(health["connected"], json!(true));
        assert_eq!(health["service_version"], json!(env!("CARGO_PKG_VERSION")));
        assert!(
            health["service"]["started_at"].is_string(),
            "the service reports when it started: {health}"
        );

        // The database: the mode the connection carries, the applied
        // schema, and when the newest timeline row was recorded.
        assert_eq!(health["database"]["journal_mode"], json!("wal"));
        assert_eq!(
            health["database"]["schema_version"],
            json!(kanban_storage::migrations::LATEST_SCHEMA_VERSION),
            "the applied schema is the latest known migration"
        );
        assert!(
            health["database"]["last_change_at"].is_string(),
            "a landed registration is a recorded change: {health}"
        );

        // The scheduler: the daily backup's persisted success time.
        assert!(
            health["scheduler"]["last_backup_success_at"].is_string(),
            "the completed startup backup is scheduler state: {health}"
        );

        // MCP: the tool surface the core exposes to adapters.
        assert_eq!(
            health["mcp"]["exposed_tools"],
            json!(EXPOSED_MCP_TOOL_NAMES.len()),
            "every catalogued MCP tool is health state"
        );

        // Herdr: the one observed session, with its own last-change
        // time — when its last full snapshot was captured.
        let sessions = health["herdr"]["sessions"]
            .as_array()
            .expect("the sessions are a list");
        assert_eq!(
            sessions[0]["project_id"],
            json!(1),
            "the session names its Project: {health}"
        );
        assert_eq!(
            sessions[0]["diagnostics"]["session_name"],
            json!("kanban-main")
        );

        // Workspaces: the health census across every Project, and
        // when a Workspace last changed.
        assert_eq!(
            health["workspaces"]["by_health"],
            json!({
                "available": 0,
                "assigned": 0,
                "dirty": 0,
                "missing": 2,
                "retired": 0,
                "unobserved": 0,
            }),
            "both absent paths hold the missing health: {health}"
        );
        assert!(
            health["workspaces"]["last_change_at"].is_string(),
            "the registrations are recorded Workspace changes: {health}"
        );

        core.shutdown();
    }

    /// KAN-T62-AC1: a fresh core answers with the same shape — absent
    /// times absent, empty censuses empty — never with invented state.
    #[test]
    fn health_get_reports_a_fresh_core_without_inventing_state() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let core = boot(&dir);
        let mut client = Client::connect(core.socket_path());

        let health = client.query("health.get");

        assert_eq!(health["connected"], json!(true));
        assert!(
            health["service"]["started_at"].is_string(),
            "the service's own start is always known: {health}"
        );
        assert_eq!(health["database"]["journal_mode"], json!("wal"));
        assert_eq!(
            health["herdr"]["sessions"],
            json!([]),
            "nothing is observed"
        );
        assert_eq!(
            health["workspaces"]["by_health"],
            json!({
                "available": 0,
                "assigned": 0,
                "dirty": 0,
                "missing": 0,
                "retired": 0,
                "unobserved": 0,
            }),
            "the census counts nothing that is not registered: {health}"
        );
        assert_eq!(
            health["database"]["last_change_at"],
            Value::Null,
            "no timeline row exists yet: {health}"
        );
        assert_eq!(
            health["workspaces"]["last_change_at"],
            Value::Null,
            "no Workspace change is recorded yet: {health}"
        );

        core.shutdown();
    }

    /// KAN-T62-AC1: Herdr state is live, not a boot snapshot — the
    /// session's connection follows the socket it dials.
    #[test]
    fn health_get_follows_a_herdr_session_disconnect() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_root = dir.path().join("sessions");
        let repository = scratch_repository(&dir, "kanban");
        let fixture = ScriptedSession::bind(
            &socket_root,
            "kanban-main",
            "/workspaces/kanban.seed",
            SessionScript::default(),
        );
        let core = serve_with_herdr_sessions(dir.path(), socket_root)
            .expect("the core boots for the test");
        let mut client = Client::connect(core.socket_path());
        client.command(
            "project.register",
            json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "health-register" },
                "code": "CORE",
                "name": "Control plane",
                "repository": repository,
                "seed_workspace": "/workspaces/kanban.seed",
                "default_branch": "main",
                "herdr_session": "kanban-main",
                "herdr_workspace": "kanban.seed",
            }),
        );

        let connected = {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let answer = client.query("health.get");
                let settled = answer["herdr"]["sessions"]
                    .as_array()
                    .is_some_and(|sessions| {
                        sessions.len() == 1
                            && sessions[0]["diagnostics"]["connected"] == json!(true)
                            && sessions[0]["diagnostics"]["last_snapshot_at"].is_string()
                    });
                if settled || Instant::now() > deadline {
                    break answer;
                }
                assert!(
                    Instant::now() < deadline,
                    "the session never connected: {answer}"
                );
                thread::sleep(Duration::from_millis(10));
            }
        };
        assert_eq!(
            connected["herdr"]["sessions"][0]["diagnostics"]["last_error"],
            Value::Null,
            "a healthy session reports no error: {connected}"
        );

        // The session's socket disappears; the observer's redial
        // marks the failure the next health query reports.
        drop(fixture);
        let disconnected = {
            let deadline = Instant::now() + Duration::from_secs(20);
            loop {
                let answer = client.query("health.get");
                let failed = answer["herdr"]["sessions"]
                    .as_array()
                    .is_some_and(|sessions| {
                        sessions.len() == 1
                            && sessions[0]["diagnostics"]["connected"] == json!(false)
                            && sessions[0]["diagnostics"]["last_error"].is_string()
                    });
                if failed || Instant::now() > deadline {
                    break answer;
                }
                assert!(
                    Instant::now() < deadline,
                    "the dropped session never reported its failure: {answer}"
                );
                thread::sleep(Duration::from_millis(10));
            }
        };
        // The last snapshot survives the failure: the last-change
        // time reports what was captured, not what is connected.
        assert!(
            disconnected["herdr"]["sessions"][0]["diagnostics"]["last_snapshot_at"].is_string(),
            "the captured snapshot outlives the connection: {disconnected}"
        );

        core.shutdown();
    }
}
