//! Static guards for the WebView's least privilege (DR-SS-09): the
//! committed configuration must never grow database, arbitrary
//! network, shell, filesystem, or secret capability, and must keep a
//! restrictive CSP.

use std::collections::HashSet;

const CONFIG: &str = include_str!("../tauri.conf.json");
const CAPABILITIES: &str = include_str!("../capabilities/default.json");
const MANIFEST: &str = include_str!("../Cargo.toml");

/// Permissions the WebView is allowed to hold: listening to the
/// shell's events and stopping that listener.
const ALLOWED_PERMISSIONS: &[&str] = &["core:event:allow-listen", "core:event:allow-unlisten"];

/// Plugins that would hand the WebView a forbidden capability.
const FORBIDDEN_PLUGINS: &[&str] = &[
    "tauri-plugin-fs",
    "tauri-plugin-shell",
    "tauri-plugin-http",
    "tauri-plugin-sql",
    "tauri-plugin-store",
    "tauri-plugin-os",
    "tauri-plugin-process",
    "tauri-plugin-notification",
    "tauri-plugin-clipboard",
    "tauri-plugin-dialog",
    "tauri-plugin-stronghold",
    "tauri-plugin-keyring",
];

fn config() -> serde_json::Value {
    serde_json::from_str(CONFIG).expect("tauri.conf.json parses")
}

fn csp_of(field: &str) -> String {
    config()["app"]["security"][field]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

/// The directives of a CSP policy, by name.
fn directives(csp: &str) -> std::collections::HashMap<String, Vec<String>> {
    let mut parsed = std::collections::HashMap::new();
    for directive in csp.split(';') {
        let mut words = directive.split_whitespace();
        let Some(name) = words.next() else { continue };
        parsed.insert(
            name.to_owned(),
            words.map(str::to_owned).collect::<Vec<_>>(),
        );
    }
    parsed
}

#[test]
fn every_csp_is_present_and_restrictive() {
    for field in ["csp", "devCsp"] {
        let csp = csp_of(field);
        assert!(!csp.is_empty(), "{field} must not be disabled");

        let directives = directives(&csp);
        let default = directives
            .get("default-src")
            .expect("default-src is set")
            .clone();
        assert_eq!(
            default,
            vec!["'self'".to_owned()],
            "{field} defaults to self"
        );

        let script = directives
            .get("script-src")
            .expect("script-src is set")
            .clone();
        assert_eq!(
            script,
            vec!["'self'".to_owned()],
            "{field} allows no script source beyond self"
        );
        assert!(
            !csp.contains("unsafe-eval"),
            "{field} must never allow unsafe-eval"
        );
        assert!(
            !csp.contains("*"),
            "{field} must never contain a wildcard source"
        );
        let style = directives.get("style-src").expect("style-src is set");
        assert!(
            style.contains(&"'self'".to_owned()),
            "{field} styles come from self"
        );
    }
}

#[test]
fn the_dev_csp_only_widens_the_dev_server() {
    let dev = directives(&csp_of("devCsp"));
    let connect = dev
        .get("connect-src")
        .expect("devCsp names connect-src")
        .clone();
    let extra: HashSet<_> = connect
        .iter()
        .filter(|source| source.as_str() != "'self'")
        .collect();
    assert_eq!(
        extra,
        HashSet::from([
            &"ipc:".to_owned(),
            &"http://ipc.localhost".to_owned(),
            &"ws://localhost:1420".to_owned(),
            &"http://localhost:1420".to_owned(),
        ]),
        "the dev policy widens nothing beyond the IPC bridge and the dev server"
    );
}

#[test]
fn the_webview_holds_no_capability_beyond_event_listening() {
    let capabilities: serde_json::Value =
        serde_json::from_str(CAPABILITIES).expect("the capability file parses");
    let permissions: Vec<String> = capabilities["permissions"]
        .as_array()
        .expect("permissions are a list")
        .iter()
        .map(|permission| {
            permission
                .as_str()
                .expect("a permission is a string")
                .to_owned()
        })
        .collect();

    for permission in &permissions {
        assert!(
            ALLOWED_PERMISSIONS.contains(&permission.as_str()),
            "permission `{permission}` is outside the allowed set"
        );
    }
    assert!(
        !permissions.is_empty(),
        "an empty capability list would mean unlisted defaults"
    );
}

#[test]
fn the_shell_links_no_capability_plugins() {
    for plugin in FORBIDDEN_PLUGINS {
        assert!(
            !MANIFEST.contains(plugin),
            "the shell must not depend on {plugin}"
        );
    }
}

/// Production dependencies the thin shell must never link (KAN-T73-AC1).
const FORBIDDEN_DEPS: &[&str] = &[
    "kanban-service",
    "kanban-storage",
    "rusqlite",
    "libsqlite3-sys",
];

/// The `[dependencies]` table body: ends at the next TOML table header, not at
/// `[` inside inline tables or feature arrays.
fn production_dependencies_section(manifest: &str) -> &str {
    let marker = "[dependencies]";
    let start = manifest.find(marker).expect("[dependencies] is present") + marker.len();
    let rest = manifest[start..]
        .strip_prefix('\n')
        .unwrap_or(&manifest[start..]);

    let mut offset = 0;
    let mut first_line = true;
    for line in rest.lines() {
        if !first_line {
            let trimmed = line.trim();
            if trimmed.starts_with('[') && trimmed.ends_with(']') && !trimmed.contains('=') {
                return rest[..offset].trim_end();
            }
        }
        first_line = false;
        offset += line.len() + 1;
    }
    rest.trim_end()
}

fn production_manifest_lists_forbidden_dependency(manifest: &str, dep: &str) -> bool {
    production_dependencies_section(manifest).contains(&format!("{dep} ="))
}

#[test]
fn dependencies_section_includes_entries_after_inline_feature_arrays() {
    let manifest = r#"
[dependencies]
serde = { version = "1.0", features = ["derive"] }
kanban-storage = { path = "crates/kanban-storage" }

[dev-dependencies]
tempfile = "3"
"#;
    let section = production_dependencies_section(manifest);
    assert!(
        section.contains("kanban-storage ="),
        "section must run through feature arrays to the next table header"
    );
    assert!(!section.contains("tempfile ="));
}

#[test]
fn forbidden_dependencies_after_feature_arrays_are_detected() {
    let manifest = r#"
[dependencies]
serde = { version = "1.0", features = ["derive"] }
kanban-storage = { path = "crates/kanban-storage" }
libsqlite3-sys = "0.30"

[dev-dependencies]
tempfile = "3"
"#;
    for dep in ["kanban-storage", "libsqlite3-sys"] {
        assert!(
            production_manifest_lists_forbidden_dependency(manifest, dep),
            "guard must see {dep} after a feature array in [dependencies]"
        );
    }
}

#[test]
fn the_shell_links_no_service_or_storage_crates() {
    let deps_section = production_dependencies_section(MANIFEST);
    for dep in FORBIDDEN_DEPS {
        assert!(
            !deps_section.contains(&format!("{dep} =")),
            "the shell must not depend on {dep}"
        );
    }
    assert!(
        !MANIFEST.contains("name = \"fake-core\""),
        "the fake core binary must not ship inside the production shell crate"
    );
}

#[test]
fn the_tauri_global_is_not_injected() {
    assert_eq!(
        config()["app"]["withGlobalTauri"],
        false,
        "the WebView must not reach the Tauri API global"
    );
}
