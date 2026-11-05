//! Issue deep links. The operating system hands the shell
//! `kanban://issue/KEY` links three ways, and the shell takes each
//! itself, through no deep-link plugin: macOS as Tauri's own open-URL
//! run event, Linux as the one argument of a launch (read by `main`,
//! see [`crate::launch`]) or of a relaunch the single-instance plugin
//! forwards here. Where the shell receives text, it judges exactly the
//! text it received, before anything parses it as a URL; macOS hands
//! Tauri a URL it has already parsed, so there the shell judges that
//! URL's text. The domain decides what a link names
//! ([`IssueKey::from_deep_link`]); the shell only announces that issue
//! to the WebView as a typed event carrying its key, holding it until
//! the WebView says it listens, so the link that launched the shell is
//! not lost. A link naming no issue changes nothing. No event carries a
//! link itself, so the WebView never receives one, never navigates to
//! one, and holds no deep-link capability.

use std::sync::{Mutex, MutexGuard};

use kanban_domain::IssueKey;
use serde::Serialize;
use tauri::plugin::TauriPlugin;
use tauri::webview::PageLoadEvent;
use tauri::{AppHandle, Emitter, Manager, RunEvent, Runtime};

/// The shell announces an issue a link names under this name.
pub const ISSUE_LINK_EVENT: &str = "shell://issue-link";

/// The window issue links open in.
const MAIN_WINDOW: &str = "main";

/// One issue a link asked the WebView to open.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IssueLinkOpened {
    /// The issue's key, `CORE-12`.
    pub identifier: String,
}

/// What became of one link the shell was handed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Received {
    /// The link names no issue, and nothing changed.
    Ignored,
    /// The WebView is not listening yet, so the issue waits for it.
    Held,
    /// The WebView is listening: announce the issue now.
    Announce(IssueLinkOpened),
}

/// The issue links the shell holds for the WebView.
#[derive(Debug, Default)]
pub struct IssueLinks {
    inbox: Mutex<Inbox>,
}

#[derive(Debug, Default)]
struct Inbox {
    /// Whether the page on show has said it listens.
    listening: bool,
    /// The newest issue that arrived while nothing listened.
    held: Option<IssueLinkOpened>,
}

impl IssueLinks {
    /// Take one link the operating system handed the shell.
    pub fn receive(&self, link: &str) -> Received {
        let Some(key) = IssueKey::from_deep_link(link) else {
            return Received::Ignored;
        };
        let opened = IssueLinkOpened {
            identifier: key.display(),
        };
        let mut inbox = self.inbox();
        if inbox.listening {
            Received::Announce(opened)
        } else {
            inbox.held = Some(opened);
            Received::Held
        }
    }

    /// The WebView listens now: hand over the issue held for it.
    pub fn listen(&self) -> Option<IssueLinkOpened> {
        let mut inbox = self.inbox();
        inbox.listening = true;
        inbox.held.take()
    }

    /// A page starts loading: links wait again until it listens.
    pub fn unlisten(&self) {
        self.inbox().listening = false;
    }

    fn inbox(&self) -> MutexGuard<'_, Inbox> {
        self.inbox
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// The shell's own issue-link plugin, which `run` builds the shell
/// with (see [`crate::shell_builder`]). It holds the link a launch
/// carried until the WebView listens, makes links wait again while a
/// page loads, and takes the links macOS's open-URL run events carry.
/// It has no commands, and emits nothing but [`ISSUE_LINK_EVENT`].
pub fn plugin<R: Runtime>(launch_link: Option<String>) -> TauriPlugin<R> {
    tauri::plugin::Builder::new("issue-link")
        .setup(move |app, _api| {
            app.manage(IssueLinks::default());
            if let Some(link) = launch_link {
                open(app, &link);
            }
            Ok(())
        })
        .on_page_load(|webview, payload| {
            if payload.event() == PageLoadEvent::Started {
                page_started(webview);
            }
        })
        .on_event(on_run_event)
        .build()
}

/// Take the links a run event carries: macOS's open-URL event, for a
/// link opened while the shell runs or the one that started it. The
/// URLs come already parsed, so their text is what the shell judges.
pub fn on_run_event<R: Runtime>(app: &AppHandle<R>, event: &RunEvent) {
    #[cfg(target_os = "macos")]
    if let RunEvent::Opened { urls } = event {
        for url in urls {
            open(app, url.as_str());
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (app, event);
}

/// A second launch handed the running shell its arguments, the
/// executable first, through the single-instance plugin: on Linux,
/// how a link reaches a shell that already runs. Its one argument is
/// judged as exactly the text it arrived as; a launch with no argument
/// only brings the window forward.
pub fn relaunched<R: Runtime>(app: &AppHandle<R>, arguments: Vec<String>, _directory: String) {
    match arguments.as_slice() {
        [] | [_] => reveal(app),
        [_, link] => open(app, link),
        _ => {}
    }
}

/// Act on one link: an issue brings the window forward and is
/// announced, or held until the WebView listens; anything else is
/// dropped untouched.
pub fn open<R: Runtime>(app: &AppHandle<R>, link: &str) {
    let Some(links) = app.try_state::<IssueLinks>() else {
        return;
    };
    match links.receive(link) {
        Received::Ignored => {}
        Received::Held => reveal(app),
        Received::Announce(opened) => {
            reveal(app);
            let _ = app.emit(ISSUE_LINK_EVENT, opened);
        }
    }
}

/// The WebView listens: announce the issue held for it, if any.
pub fn listen<R: Runtime>(app: &AppHandle<R>) {
    let held = app
        .try_state::<IssueLinks>()
        .and_then(|links| links.listen());
    if let Some(opened) = held {
        let _ = app.emit(ISSUE_LINK_EVENT, opened);
    }
}

/// A page starts loading in a webview: until it listens, links wait.
pub fn page_started<R: Runtime>(webview: &tauri::Webview<R>) {
    if let Some(links) = webview.try_state::<IssueLinks>() {
        links.unlisten();
    }
}

/// The scheme every link the shell claims starts with.
const LINK_SCHEME: &str = "kanban:";

/// Whether a launch argument is a `kanban:` link, which Linux hands
/// the shell as its one argument. This only tells a link launch from
/// a mistyped flag: the domain decides what the link names.
pub fn is_link_argument(argument: &std::ffi::OsStr) -> bool {
    argument
        .to_str()
        .and_then(|text| text.get(..LINK_SCHEME.len()))
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case(LINK_SCHEME))
}

/// Bring the main window forward.
pub fn reveal<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Make this executable the `kanban` scheme's handler where that
/// happens at runtime: on Linux, where only an installed package's
/// desktop entry would otherwise declare it (see
/// [`write_scheme_handler`]). It runs off the main thread, so the
/// window does not wait for the desktop database. macOS reads the
/// scheme from the bundle's `Info.plist` instead.
pub fn register_scheme<R: Runtime>(app: &AppHandle<R>) {
    #[cfg(target_os = "linux")]
    {
        let name = app.config().product_name.clone();
        let appimage = app.env().appimage.map(std::path::PathBuf::from);
        let applications = app.path().data_dir().map(|data| data.join("applications"));
        let spawned = std::thread::Builder::new()
            .name("kanban-scheme-registration".to_owned())
            .spawn(move || {
                let registered =
                    applications
                        .map_err(std::io::Error::other)
                        .and_then(|applications| {
                            let executable = tauri::utils::platform::current_exe()?;
                            let launcher = appimage.unwrap_or_else(|| executable.clone());
                            write_scheme_handler(
                                &applications,
                                &executable,
                                &launcher,
                                name.as_deref(),
                            )
                        });
                if let Err(failure) = registered {
                    eprintln!("kanban shell: the kanban:// scheme was not registered: {failure}");
                }
            });
        if let Err(failure) = spawned {
            eprintln!("kanban shell: the kanban:// scheme was not registered: {failure}");
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = app;
}

/// Write the desktop entry that makes `launcher` the `kanban` scheme's
/// handler into `applications`, named after `executable`
/// (`kanban-desktop-handler.desktop`), then refresh the desktop
/// database there and make the entry `xdg-mime`'s default for the
/// scheme. Either command missing or failing is reported.
#[cfg(target_os = "linux")]
pub fn write_scheme_handler(
    applications: &std::path::Path,
    executable: &std::path::Path,
    launcher: &std::path::Path,
    name: Option<&str>,
) -> std::io::Result<()> {
    let file_name = format!(
        "{}-handler.desktop",
        executable.file_name().unwrap_or_default().to_string_lossy()
    );
    let mime_type = format!("x-scheme-handler/{}", LINK_SCHEME.trim_end_matches(':'));
    let entry = format!(
        "[Desktop Entry]\nType=Application\nName={}\nExec=\"{}\" %u\nTerminal=false\nMimeType={mime_type};\nNoDisplay=true\n",
        name.unwrap_or(&file_name),
        launcher.to_string_lossy(),
    );
    std::fs::create_dir_all(applications)?;
    let path = applications.join(&file_name);
    if std::fs::read_to_string(&path).ok().as_deref() != Some(entry.as_str()) {
        std::fs::write(&path, entry)?;
    }
    let succeeded = |program: &str, arguments: &[&std::ffi::OsStr]| {
        let status = std::process::Command::new(program)
            .args(arguments)
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(std::io::Error::other(format!("{program} {status}")))
        }
    };
    succeeded("update-desktop-database", &[applications.as_os_str()])?;
    succeeded(
        "xdg-mime",
        &["default".as_ref(), file_name.as_ref(), mime_type.as_ref()],
    )
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    use super::{IssueLinkOpened, IssueLinks, Received, is_link_argument};

    #[test]
    fn a_launch_argument_with_the_kanban_scheme_is_a_link() {
        for argument in [
            "kanban://issue/CORE-12",
            "kanban://issue/core-12",
            "kanban://board",
            "KANBAN://issue/CORE-12",
        ] {
            assert!(is_link_argument(OsStr::new(argument)), "{argument}");
        }
    }

    #[test]
    fn any_other_launch_argument_is_not_a_link() {
        for argument in [
            "",
            "kanban",
            "kanban-dev://issue/CORE-12",
            "--version",
            "--package-smoke",
            "https://example.com/kanban://issue/CORE-12",
            "/tmp/kanban://issue/CORE-12",
            " kanban://issue/CORE-12",
        ] {
            assert!(!is_link_argument(OsStr::new(argument)), "{argument}");
        }
        assert!(
            !is_link_argument(OsStr::from_bytes(b"kanban://issue/\xff")),
            "an argument that is not text is no link"
        );
    }

    fn opened(identifier: &str) -> IssueLinkOpened {
        IssueLinkOpened {
            identifier: identifier.to_owned(),
        }
    }

    #[test]
    fn a_link_before_the_webview_listens_waits_for_it() {
        let links = IssueLinks::default();

        assert_eq!(links.receive("kanban://issue/CORE-12"), Received::Held);
        assert_eq!(links.listen(), Some(opened("CORE-12")));
        assert_eq!(links.listen(), None, "a held issue is handed over once");
    }

    #[test]
    fn a_link_while_the_webview_listens_is_announced_at_once() {
        let links = IssueLinks::default();
        assert_eq!(links.listen(), None);

        assert_eq!(
            links.receive("kanban://issue/CORE-12"),
            Received::Announce(opened("CORE-12"))
        );
        assert_eq!(links.listen(), None, "an announced issue is not held");
    }

    #[test]
    fn only_the_newest_waiting_link_is_kept() {
        let links = IssueLinks::default();

        links.receive("kanban://issue/CORE-12");
        links.receive("kanban://issue/EDGE-3");

        assert_eq!(links.listen(), Some(opened("EDGE-3")));
    }

    #[test]
    fn a_link_naming_no_issue_changes_nothing() {
        let links = IssueLinks::default();
        links.receive("kanban://issue/CORE-12");

        for link in [
            "kanban://issue/core-12",
            "kanban://issue/CORE-12/edit",
            "kanban://issue/CORE-12?x=1",
            "https://example.com/issue/CORE-12",
            "kanban://evil/CORE-12",
            "kanban://issue/",
        ] {
            assert_eq!(links.receive(link), Received::Ignored, "{link}");
        }

        assert_eq!(
            links.listen(),
            Some(opened("CORE-12")),
            "a refused link leaves the held issue alone"
        );
        assert_eq!(
            links.receive("kanban://issue/CORE-12#x"),
            Received::Ignored,
            "a refused link announces nothing while the WebView listens"
        );
    }

    #[test]
    fn a_page_load_makes_links_wait_again() {
        let links = IssueLinks::default();
        links.listen();

        links.unlisten();

        assert_eq!(links.receive("kanban://issue/CORE-12"), Received::Held);
        assert_eq!(links.listen(), Some(opened("CORE-12")));
    }

    #[test]
    fn an_announcement_carries_only_the_key() {
        let payload = serde_json::to_value(opened("CORE-12")).expect("the announcement encodes");

        assert_eq!(payload, serde_json::json!({ "identifier": "CORE-12" }));
    }
}
