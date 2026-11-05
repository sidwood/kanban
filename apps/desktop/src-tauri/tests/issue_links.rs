//! Issue deep links through the shell exactly as `run` builds it
//! ([`kanban_desktop_lib::shell_builder`]), on Tauri's mock runtime with
//! the committed configuration. However the operating system hands the
//! shell a link (a launch's argument, a relaunch's arguments, or macOS's
//! open-URL event), the WebView hears of it only as a typed event
//! carrying the key it names. A link that arrives before the WebView
//! listens waits for it, a link naming no issue changes nothing, and no
//! other event carries a link at all.

use std::ffi::OsString;
use std::sync::{Arc, Mutex};

use kanban_desktop_lib::issue_link::{self, ISSUE_LINK_EVENT};
use kanban_desktop_lib::launch::{self, Launch};
use serde_json::{Value, json};
use tauri::Listener;
use tauri::test::MockRuntime;

const CONFIG: &str = include_str!("../tauri.conf.json");
const LOCKFILE: &str = include_str!("../Cargo.lock");

/// The production entry's source. `main` and `run` start a real
/// window and core, so their wiring is read here rather than run.
const MAIN_SOURCE: &str = include_str!("../src/main.rs");
const SHELL_SOURCE: &str = include_str!("../src/lib.rs");
const ISSUE_LINK_SOURCE: &str = include_str!("../src/issue_link.rs");

/// The event the deep-link plugin announces every incoming link on,
/// raw, to anything that listens: a WebView holding the ordinary
/// event-listening permission included.
const PLUGIN_EVENT: &str = "deep-link://new-url";

/// Links the domain refuses as they stand.
const REFUSED: &[&str] = &[
    "kanban://issue/CORE-12?x=1",
    "kanban://issue/core-12",
    "kanban://issue/CORE-12/edit",
    "kanban://issue/CORE-12#comments",
    "kanban://board/CORE-12",
    "https://example.com/issue/CORE-12",
];

/// Text that names no issue as it arrives, but that URL parsing would
/// rewrite into `kanban://issue/CORE-12`.
const NORMALISED: &[&str] = &[
    "kanban://issue/x/../CORE-12",
    "kanban://issue/%2e/CORE-12",
    "kanban://issue/CO\nRE-12",
    "kanban://issue/CORE-12 ",
];

struct Shell {
    app: tauri::App<MockRuntime>,
    webview: tauri::WebviewWindow<MockRuntime>,
    announced: Arc<Mutex<Vec<Value>>>,
    plugin_events: Arc<Mutex<Vec<String>>>,
}

fn config() -> Value {
    serde_json::from_str(CONFIG).expect("tauri.conf.json parses")
}

/// The shell `run` builds, holding the link it was launched with,
/// configured by `tauri.conf.json`'s plugin section, and recording every
/// issue link it announces and every plugin announcement of a link.
fn shell(launch_link: Option<String>) -> Shell {
    let mut context = tauri::test::mock_context(tauri::test::noop_assets());
    for (name, section) in config()["plugins"]
        .as_object()
        .expect("tauri.conf.json configures plugins")
    {
        context
            .config_mut()
            .plugins
            .0
            .insert(name.clone(), section.clone());
    }
    let app = kanban_desktop_lib::shell_builder(tauri::test::mock_builder(), launch_link)
        .build(context)
        .expect("the mock shell app builds");
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("the mock webview builds");
    let announced = Arc::new(Mutex::new(Vec::new()));
    let sink = announced.clone();
    app.listen_any(ISSUE_LINK_EVENT, move |event| {
        let payload = serde_json::from_str(event.payload()).expect("the announcement is JSON");
        sink.lock().expect("the sink is not poisoned").push(payload);
    });
    let plugin_events = Arc::new(Mutex::new(Vec::new()));
    let sink = plugin_events.clone();
    app.listen_any(PLUGIN_EVENT, move |event| {
        sink.lock()
            .expect("the sink is not poisoned")
            .push(event.payload().to_owned());
    });
    Shell {
        app,
        webview,
        announced,
        plugin_events,
    }
}

/// The link a launch with this one argument hands the shell, read the
/// way `main` reads it.
fn launched_with(argument: &str) -> Option<String> {
    match launch::read(&[OsString::from(argument)]) {
        Launch::Shell { link } => link,
        other => panic!("`{argument}` must start the shell, not {other:?}"),
    }
}

impl Shell {
    fn announced(&self) -> Vec<Value> {
        self.announced
            .lock()
            .expect("the sink is not poisoned")
            .clone()
    }

    fn plugin_events(&self) -> Vec<String> {
        self.plugin_events
            .lock()
            .expect("the sink is not poisoned")
            .clone()
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

    /// A second launch hands the running shell its arguments, as the
    /// single-instance plugin passes them on: the executable first.
    fn relaunch(&self, arguments: &[&str]) {
        issue_link::relaunched(
            self.app.handle(),
            arguments
                .iter()
                .map(|argument| (*argument).to_owned())
                .collect(),
            String::new(),
        );
    }

    /// macOS hands the running shell links as an open-URL run event.
    #[cfg(target_os = "macos")]
    fn open_urls(&self, links: &[&str]) {
        let urls = links
            .iter()
            .map(|link| tauri::Url::parse(link).expect("the operating system hands over URLs"))
            .collect();
        issue_link::on_run_event(self.app.handle(), &tauri::RunEvent::Opened { urls });
    }
}

fn opened(identifier: &str) -> Value {
    json!({ "identifier": identifier })
}

#[test]
fn the_bundle_declares_the_kanban_scheme() {
    assert_eq!(
        config()["plugins"]["deep-link"],
        json!({ "desktop": { "schemes": ["kanban"] } }),
        "the bundler declares exactly the scheme the core mints links under"
    );
}

#[test]
fn a_link_a_launch_carries_is_announced_once_the_webview_listens() {
    let shell = shell(launched_with("kanban://issue/CORE-12"));
    assert!(shell.announced().is_empty(), "nothing listens yet");

    shell.listen();
    assert_eq!(shell.announced(), vec![opened("CORE-12")]);

    shell.listen();
    assert_eq!(shell.announced().len(), 1, "a held link is announced once");
}

#[test]
fn a_shell_launched_without_a_link_announces_nothing() {
    let shell = shell(None);

    shell.listen();

    assert!(shell.announced().is_empty());
}

#[test]
fn a_relaunch_link_reaches_a_listening_webview_at_once() {
    let shell = shell(None);
    shell.listen();

    shell.relaunch(&["kanban", "kanban://issue/CORE-12"]);
    shell.relaunch(&["kanban", "kanban://issue/EDGE-3"]);

    assert_eq!(shell.announced(), vec![opened("CORE-12"), opened("EDGE-3")]);
}

#[test]
fn a_relaunch_link_before_the_webview_listens_waits_for_it() {
    let shell = shell(None);

    shell.relaunch(&["kanban", "kanban://issue/EDGE-3"]);
    shell.relaunch(&["kanban"]);
    assert!(shell.announced().is_empty(), "nothing listens yet");

    shell.listen();
    assert_eq!(shell.announced(), vec![opened("EDGE-3")]);
}

#[test]
fn a_link_naming_no_issue_announces_nothing() {
    for link in REFUSED {
        // `main` refuses an argument of another scheme before the
        // shell starts; the shell refuses whatever it is handed.
        let shell = shell(Some((*link).to_owned()));
        shell.listen();
        shell.relaunch(&["kanban", link]);

        assert!(
            shell.announced().is_empty(),
            "`{link}` is neither held nor announced"
        );
    }
}

#[test]
fn a_link_is_judged_as_the_text_it_arrived_as() {
    for link in NORMALISED {
        assert_eq!(
            tauri::Url::parse(link)
                .expect("URL parsing accepts the text")
                .as_str(),
            "kanban://issue/CORE-12",
            "URL parsing rewrites {link:?} into a valid link"
        );

        let shell = shell(launched_with(link));
        shell.listen();
        assert!(
            shell.announced().is_empty(),
            "a launch with {link:?} announces nothing"
        );

        shell.relaunch(&["kanban", link]);
        assert!(
            shell.announced().is_empty(),
            "a relaunch with {link:?} announces nothing"
        );
    }
}

#[test]
fn only_the_key_of_a_valid_link_ever_reaches_the_webview() {
    let shell = shell(launched_with("kanban://issue/CORE-12?x=1"));
    shell.relaunch(&["kanban", "kanban://issue/x/../CORE-12"]);
    shell.relaunch(&["kanban", "kanban://issue/EDGE-3"]);
    shell.listen();
    shell.relaunch(&["kanban", "kanban://issue/CORE-12?token=secret"]);
    shell.relaunch(&["kanban", "kanban://issue/CORE-12"]);
    #[cfg(target_os = "macos")]
    shell.open_urls(&["kanban://issue/CORE-12?x=1", "kanban://issue/AB-1"]);

    let mut expected = vec![opened("EDGE-3"), opened("CORE-12")];
    if cfg!(target_os = "macos") {
        expected.push(opened("AB-1"));
    }
    assert_eq!(
        shell.announced(),
        expected,
        "each valid link is announced as its key alone"
    );
    assert_eq!(
        shell.plugin_events(),
        Vec::<String>::new(),
        "no plugin announces an incoming link"
    );
    assert!(
        !LOCKFILE.contains("name = \"tauri-plugin-deep-link\""),
        "the shell links no plugin that announces incoming links to the WebView"
    );
}

/// macOS hands Tauri links already parsed, so the shell judges the
/// text of the URL it was given.
#[cfg(target_os = "macos")]
#[test]
fn a_macos_open_url_event_announces_the_key_it_names() {
    let shell = shell(None);
    shell.open_urls(&["kanban://issue/CORE-12"]);
    assert!(shell.announced().is_empty(), "nothing listens yet");

    shell.listen();
    shell.open_urls(&[
        "kanban://issue/CORE-12?tab=history",
        "kanban://issue/EDGE-3",
    ]);

    assert_eq!(shell.announced(), vec![opened("CORE-12"), opened("EDGE-3")]);
}

/// The function `signature` opens in `source`, short of its closing
/// brace and without its comment lines, with whitespace kept only as
/// one space between two words, so neither formatting nor a
/// commented-out call changes what it is read as.
fn code_of(source: &str, signature: &str) -> String {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("`{signature}` is defined"));
    let function = &source[start..];
    let end = function
        .find("\n}\n")
        .unwrap_or_else(|| panic!("`{signature}` closes"));
    let word = |character: char| character.is_alphanumeric() || character == '_';
    let mut code = String::new();
    for token in function[..end]
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .flat_map(str::split_whitespace)
    {
        if code.ends_with(word) && token.starts_with(word) {
            code.push(' ');
        }
        code.push_str(token);
    }
    code
}

/// The tests above drive the shell `shell_builder` builds. This holds
/// the production entry to that same shell: `main` hands `run` the
/// link a launch carried, `run` builds through `shell_builder` with
/// that link and with the single-instance plugin handing a relaunch's
/// arguments to the shell, and the issue-link plugin takes the run
/// events macOS's open-URL links arrive as, which Tauri's mock
/// runtime cannot raise.
#[test]
fn the_production_entry_delivers_links_through_the_shell_these_tests_drive() {
    let main = code_of(MAIN_SOURCE, "fn main()");
    assert!(
        main.contains(
            "let arguments:Vec<_>=std::env::args_os().skip(1).collect();\
             match launch::read(&arguments){Launch::Shell{link}=>{kanban_desktop_lib::run(link)"
        ),
        "main hands run the link a launch carried: {main}"
    );

    let run = code_of(SHELL_SOURCE, "pub fn run(");
    let built = "pub fn run(launch_link:Option<String>)->tauri::Result<()>{\
                 shell_builder(tauri::Builder::default()\
                 .plugin(tauri_plugin_single_instance::init(issue_link::relaunched)),\
                 launch_link,)";
    assert!(
        run.strip_prefix(built)
            .is_some_and(|rest| rest.starts_with('.')),
        "run builds on the shell shell_builder builds, holding its launch link \
         and handing a relaunch's arguments to the shell: {run}"
    );
    assert!(
        run.ends_with(".run(tauri::generate_context!())"),
        "run runs the shell it built: {run}"
    );

    let plugin = code_of(ISSUE_LINK_SOURCE, "pub fn plugin<");
    assert!(
        plugin.contains(".on_event(on_run_event)"),
        "the issue-link plugin takes macOS's open-URL run events: {plugin}"
    );
}
