//! Issue deep links through the shell, on Tauri's mock runtime with the
//! real deep-link plugin and the committed scheme configuration: a link
//! reaches the WebView only as a typed event carrying the key it names,
//! a link that arrives before the WebView listens waits for it, and a
//! link naming no issue changes nothing.

use std::sync::{Arc, Mutex};

use kanban_desktop_lib::issue_link::{self, ISSUE_LINK_EVENT, IssueLinks};
use serde_json::{Value, json};
use tauri::test::MockRuntime;
use tauri::{Emitter, Listener};

const CONFIG: &str = include_str!("../tauri.conf.json");

/// The deep-link plugin's own announcement of a link, which macOS's
/// open-URL event and a Linux launch's argument both become.
const PLUGIN_EVENT: &str = "deep-link://new-url";

struct Shell {
    app: tauri::App<MockRuntime>,
    webview: tauri::WebviewWindow<MockRuntime>,
    announced: Arc<Mutex<Vec<Value>>>,
}

fn config() -> Value {
    serde_json::from_str(CONFIG).expect("tauri.conf.json parses")
}

/// A shell with the deep-link plugin configured as `tauri.conf.json`
/// configures it, recording every issue link it announces.
fn shell() -> Shell {
    let mut context = tauri::test::mock_context(tauri::test::noop_assets());
    context.config_mut().plugins.0.insert(
        "deep-link".to_owned(),
        config()["plugins"]["deep-link"].clone(),
    );
    let app = tauri::test::mock_builder()
        .plugin(tauri_plugin_deep_link::init())
        .manage(IssueLinks::default())
        .invoke_handler(kanban_desktop_lib::catalogue_invoke_handler())
        .build(context)
        .expect("the mock shell app builds");
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("the mock webview builds");
    let announced = Arc::new(Mutex::new(Vec::new()));
    let sink = announced.clone();
    app.listen(ISSUE_LINK_EVENT, move |event| {
        let payload = serde_json::from_str(event.payload()).expect("the announcement is JSON");
        sink.lock().expect("the sink is not poisoned").push(payload);
    });
    Shell {
        app,
        webview,
        announced,
    }
}

impl Shell {
    fn announced(&self) -> Vec<Value> {
        self.announced
            .lock()
            .expect("the sink is not poisoned")
            .clone()
    }

    /// The deep-link plugin hands the shell links.
    fn open(&self, links: &[&str]) {
        self.app
            .emit(PLUGIN_EVENT, links)
            .expect("the plugin event emits");
    }

    /// The WebView's one call: it is listening now.
    fn listen(&self) {
        let message = tauri::webview::InvokeRequest {
            cmd: "issue_link_listen".to_owned(),
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url: "tauri://localhost".parse().expect("the mock origin parses"),
            body: tauri::ipc::InvokeBody::Json(json!({})),
            headers: Default::default(),
            invoke_key: tauri::test::INVOKE_KEY.to_owned(),
        };
        tauri::test::get_ipc_response(&self.webview, message)
            .expect("the WebView may say it listens");
    }
}

fn opened(identifier: &str) -> Value {
    json!({ "identifier": identifier })
}

#[test]
fn the_desktop_shell_registers_the_kanban_scheme() {
    assert_eq!(
        config()["plugins"]["deep-link"],
        json!({ "desktop": { "schemes": ["kanban"] } }),
        "the shell claims exactly the scheme the core mints links under"
    );
}

#[test]
fn a_link_before_the_webview_listens_is_announced_once_it_does() {
    let shell = shell();
    issue_link::attach(shell.app.handle());

    shell.open(&["kanban://issue/CORE-12"]);
    assert!(shell.announced().is_empty(), "nothing listens yet");

    shell.listen();
    assert_eq!(shell.announced(), vec![opened("CORE-12")]);

    shell.listen();
    assert_eq!(shell.announced().len(), 1, "a held link is announced once");
}

#[test]
fn a_link_reaches_a_listening_webview_at_once() {
    let shell = shell();
    issue_link::attach(shell.app.handle());
    shell.listen();

    shell.open(&["kanban://issue/CORE-12"]);
    shell.open(&["kanban://issue/EDGE-3"]);

    assert_eq!(shell.announced(), vec![opened("CORE-12"), opened("EDGE-3")]);
}

#[test]
fn a_link_naming_no_issue_announces_nothing() {
    let shell = shell();
    issue_link::attach(shell.app.handle());

    shell.open(&[
        "kanban://issue/core-12",
        "kanban://issue/CORE-12/edit",
        "kanban://issue/CORE-12?token=secret",
        "kanban://board/CORE-12",
        "https://example.com/issue/CORE-12",
    ]);
    shell.listen();
    shell.open(&["kanban://issue/", "kanban://issue/CORE-12#comments"]);

    assert!(
        shell.announced().is_empty(),
        "a link naming no issue is neither held nor announced"
    );
}

#[test]
fn the_webview_hears_only_the_key_of_a_valid_link() {
    let shell = shell();
    issue_link::attach(shell.app.handle());
    shell.listen();

    shell.open(&["https://example.com", "kanban://issue/CORE-12"]);

    assert_eq!(shell.announced(), vec![opened("CORE-12")]);
}

/// On Linux a link launches the shell as its one argument, which the
/// plugin reads as it starts, before anything listens.
#[cfg(target_os = "linux")]
#[test]
fn the_link_a_linux_launch_carries_waits_for_the_webview() {
    use tauri_plugin_deep_link::DeepLinkExt;

    let shell = shell();
    shell
        .app
        .deep_link()
        .handle_cli_arguments(["kanban", "kanban://issue/CORE-12"].into_iter());
    issue_link::attach(shell.app.handle());
    assert!(shell.announced().is_empty(), "nothing listens yet");

    shell.listen();

    assert_eq!(shell.announced(), vec![opened("CORE-12")]);
}

/// A second launch hands its arguments to the running shell through
/// the single-instance plugin, which passes them to the deep-link
/// plugin exactly like this.
#[cfg(target_os = "linux")]
#[test]
fn a_linux_relaunch_with_a_link_reaches_the_running_shell() {
    use tauri_plugin_deep_link::DeepLinkExt;

    let shell = shell();
    issue_link::attach(shell.app.handle());
    shell.listen();

    shell
        .app
        .deep_link()
        .handle_cli_arguments(["kanban", "kanban://issue/EDGE-3"].into_iter());
    shell
        .app
        .deep_link()
        .handle_cli_arguments(["kanban", "kanban://issue/edge-3"].into_iter());

    assert_eq!(shell.announced(), vec![opened("EDGE-3")]);
}
