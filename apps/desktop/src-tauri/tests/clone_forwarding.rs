//! The clone operations' Tauri exposure, driven through the shell's
//! real command registration on Tauri's mock runtime: the catalogued
//! handlers forward the generated request through the core's guarded
//! command path unchanged, unknown request fields are refused at the
//! invoke boundary before anything reaches the core, and no
//! uncatalogued clone command is reachable (KAN-T115, KAN-S6-US4).

use std::sync::{Arc, Mutex};

use kanban_app::{
    CommandEffects, CommandHandler, Core, MemoryIdempotencyStore, ParsedCommand, exposed_operations,
};
use kanban_desktop_lib::Shell;
use kanban_desktop_lib::commands::install_link;
use kanban_transport::SocketServer;
use serde_json::{Value, json};
use tauri::Manager;
use tempfile::TempDir;

/// What crossed the core's command guard, in order.
#[derive(Default)]
struct Recorder {
    seen: Mutex<Vec<(&'static str, Value)>>,
}

impl Recorder {
    fn push(&self, operation: &'static str, payload: &Value) {
        self.seen
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push((operation, payload.clone()));
    }

    fn log(&self) -> Vec<(&'static str, Value)> {
        self.seen
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

/// One guarded clone command: it records the payload that crossed the
/// mutation guard and answers with its scripted record.
struct RecordingCloneCommand {
    recorder: Arc<Recorder>,
    operation: &'static str,
    answer: Value,
}

impl CommandHandler for RecordingCloneCommand {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, kanban_dto::ApiError> {
        ParsedCommand::lift("clone-recording", payload)
    }

    fn current_version(&self, _command: &ParsedCommand) -> Result<u64, kanban_dto::ApiError> {
        Ok(0)
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        _effects: &dyn CommandEffects,
    ) -> Result<Value, kanban_dto::ApiError> {
        self.recorder.push(self.operation, &command.payload);
        Ok(self.answer.clone())
    }
}

fn create_request() -> Value {
    json!({
        "mutation": {
            "optimistic_version": 0,
            "idempotency_key": "key-clone-create",
        },
        "project_id": 1,
        "path": "/workspaces/kanban.fleet-kan-t115",
        "branch": "fleet/kan-t115",
    })
}

fn remove_request() -> Value {
    json!({
        "mutation": {
            "optimistic_version": 0,
            "idempotency_key": "key-clone-remove",
        },
        "workspace_id": 1,
    })
}

fn created_answer() -> Value {
    json!({
        "project_id": 1,
        "path": "/workspaces/kanban.fleet-kan-t115",
        "branch": "fleet/kan-t115",
    })
}

fn removed_answer() -> Value {
    json!({
        "project_id": 1,
        "workspace_id": 1,
        "path": "/workspaces/kanban.fleet-kan-t115",
        "branch": "fleet/kan-t115",
    })
}

/// A mock shell app managing a shell whose link points at a core
/// serving the two guarded clone commands. The invoke handler is the
/// production catalogue, so dispatch, argument decoding, and handler
/// bodies are the real ones.
fn clone_shell_with_core(
    dir: &TempDir,
    recorder: Arc<Recorder>,
) -> (
    kanban_transport::ServerHandle,
    tauri::App<tauri::test::MockRuntime>,
    tauri::WebviewWindow<tauri::test::MockRuntime>,
) {
    let server = SocketServer::bind(dir.path()).expect("the server binds");
    let mut core = Core::new(
        exposed_operations(),
        Arc::new(MemoryIdempotencyStore::new()),
        server.broker(),
    );
    core.register_command(
        "clone.create",
        Arc::new(RecordingCloneCommand {
            recorder: recorder.clone(),
            operation: "clone.create",
            answer: created_answer(),
        }),
    )
    .expect("clone.create registers");
    core.register_command(
        "clone.remove",
        Arc::new(RecordingCloneCommand {
            recorder,
            operation: "clone.remove",
            answer: removed_answer(),
        }),
    )
    .expect("clone.remove registers");
    let handle = server.serve(Arc::new(core)).expect("the server serves");

    let shell = Arc::new(Shell::default());
    let link = kanban_desktop_lib::core_link::CoreLink::connect(handle.socket_path())
        .expect("the link connects");
    install_link(&shell, link);

    let app = tauri::test::mock_builder()
        .invoke_handler(kanban_desktop_lib::catalogue_invoke_handler())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("the mock shell app builds");
    app.manage(shell);
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("the mock webview builds");
    (handle, app, webview)
}

/// One WebView invoke, shaped as the generated client sends it: the
/// request object under the handler's `request` argument.
fn invoke(
    webview: &tauri::WebviewWindow<tauri::test::MockRuntime>,
    cmd: &str,
    request: Value,
) -> Result<Value, Value> {
    let message = tauri::webview::InvokeRequest {
        cmd: cmd.to_owned(),
        callback: tauri::ipc::CallbackFn(0),
        error: tauri::ipc::CallbackFn(1),
        url: "tauri://localhost".parse().expect("the mock origin parses"),
        body: tauri::ipc::InvokeBody::Json(json!({ "request": request })),
        headers: Default::default(),
        invoke_key: tauri::test::INVOKE_KEY.to_owned(),
    };
    tauri::test::get_ipc_response(webview, message)
        .map(|body| body.deserialize::<Value>().expect("the answer parses"))
}

#[test]
fn clone_commands_forward_requests_through_the_guarded_core_path() {
    let dir = TempDir::new().expect("a scratch directory is available");
    let recorder = Arc::new(Recorder::default());
    let (handle, _app, webview) = clone_shell_with_core(&dir, recorder.clone());

    let created = invoke(&webview, "clone_create", create_request()).expect("clone_create answers");
    assert_eq!(created, created_answer());
    let removed = invoke(&webview, "clone_remove", remove_request()).expect("clone_remove answers");
    assert_eq!(removed, removed_answer());

    // The handlers forward the generated requests unchanged, each
    // through its one guarded core command, and nothing else crosses
    // the link.
    assert_eq!(
        recorder.log(),
        vec![
            ("clone.create", create_request()),
            ("clone.remove", remove_request()),
        ],
        "the clone handlers must only forward their own request through the guarded core command path"
    );

    handle.shutdown();
}

#[test]
fn clone_commands_refuse_unknown_request_fields() {
    let dir = TempDir::new().expect("a scratch directory is available");
    let recorder = Arc::new(Recorder::default());
    let (handle, _app, webview) = clone_shell_with_core(&dir, recorder.clone());

    for (cmd, request) in [
        ("clone_create", create_request()),
        ("clone_remove", remove_request()),
    ] {
        let mut request = request;
        request
            .as_object_mut()
            .expect("clone requests are objects")
            .insert("definitely_unknown".to_owned(), json!(true));
        let refused = invoke(&webview, cmd, request);
        assert!(refused.is_err(), "{cmd} must refuse unknown request fields");
    }

    assert!(
        recorder.log().is_empty(),
        "a refused boundary request must never reach the guarded command path"
    );

    handle.shutdown();
}

#[test]
fn uncatalogued_clone_commands_are_not_reachable() {
    let dir = TempDir::new().expect("a scratch directory is available");
    let recorder = Arc::new(Recorder::default());
    let (handle, _app, webview) = clone_shell_with_core(&dir, recorder.clone());

    for cmd in ["clone_delete", "clone_list", "clone_create_v2"] {
        let refused = invoke(&webview, cmd, create_request())
            .expect_err(&format!("{cmd} must not be a reachable command"));
        assert!(
            refused
                .as_str()
                .is_some_and(|message| message.contains("not found")),
            "{cmd} must be rejected as an unknown command, not reach a handler: {refused}"
        );
    }

    assert!(
        recorder.log().is_empty(),
        "an unreachable command must never reach the core"
    );

    handle.shutdown();
}
