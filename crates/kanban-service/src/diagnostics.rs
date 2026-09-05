//! The exportable diagnostic bundle: logs, health, and configuration
//! assembled under managed application data with redaction applied at
//! assembly (KAN-S13-US4, DR-RB-11, DR-SS-15).
//!
//! Export copies the structured logs beside a snapshot of the current
//! health answer and the managed configuration. Nothing carries a
//! secret out of the bundle: the redactor built from the managed
//! configuration scrubs every artifact as it is written, so a secret
//! planted anywhere the core could echo it still never appears in
//! the exported tree. A configuration that cannot be read or parsed
//! refuses the whole export — unsafe data must not be exported — so
//! the redaction source is checked before anything is created.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use kanban_app::QueryHandler;
use kanban_dto::{ApiError, DiagnosticsExportQuery, DiagnosticsExportResponse, HealthResponse};
use serde_json::Value;

use kanban_storage::paths::{diagnostics_dir, logs_dir};

use crate::redaction::{RedactionSourceError, Redactor, read_managed_config};

/// Why a diagnostic bundle could not be exported.
#[derive(Debug, thiserror::Error)]
pub enum DiagnosticsError {
    /// An artifact could not be read or written.
    #[error("the diagnostic bundle could not touch {path}: {source}")]
    Io {
        /// The path that refused.
        path: PathBuf,
        /// The underlying failure.
        source: std::io::Error,
    },
    /// An artifact could not be encoded.
    #[error("the diagnostic bundle could not encode an artifact: {source}")]
    Encode {
        /// The underlying failure.
        source: serde_json::Error,
    },
    /// The managed configuration could not be read or parsed, so the
    /// bundle's redaction cannot be trusted.
    #[error("the diagnostic bundle refuses to export over an unusable configuration: {source}")]
    Redaction {
        /// The underlying failure.
        source: RedactionSourceError,
    },
}

/// Exports one diagnostic bundle under `diagnostics/<bundle id>/`
/// holding the redacted logs, the redacted health snapshot, and the
/// redacted managed configuration, and returns the bundle directory.
pub fn export_diagnostic_bundle(
    data_dir: &Path,
    health: &Value,
) -> Result<PathBuf, DiagnosticsError> {
    // Fail closed first: a configuration that cannot feed the
    // redactor must refuse the export before any bundle directory
    // exists to hold unredacted data.
    let configuration =
        read_managed_config(data_dir).map_err(|source| DiagnosticsError::Redaction { source })?;
    let redactor = match &configuration {
        Some(value) => Redactor::from_config_json(value),
        None => Redactor::default(),
    };

    let bundle = diagnostics_dir(data_dir).join(unix_millis().to_string());
    fs::create_dir_all(&bundle).map_err(|source| DiagnosticsError::Io {
        path: bundle.clone(),
        source,
    })?;

    write_artifact(
        &bundle.join("health.json"),
        &encode_artifact(&redactor.redact_json(health))?,
    )?;

    let configuration = configuration
        .map(|value| redactor.redact_json(&value))
        .unwrap_or_else(|| serde_json::json!({ "present": false }));
    write_artifact(
        &bundle.join("config.json"),
        &encode_artifact(&configuration)?,
    )?;

    let source_logs = logs_dir(data_dir);
    if source_logs.is_dir() {
        let bundle_logs = bundle.join("logs");
        fs::create_dir_all(&bundle_logs).map_err(|source| DiagnosticsError::Io {
            path: bundle_logs.clone(),
            source,
        })?;
        for entry in fs::read_dir(&source_logs).map_err(|source| DiagnosticsError::Io {
            path: source_logs.clone(),
            source,
        })? {
            let path = entry
                .map_err(|source| DiagnosticsError::Io {
                    path: source_logs.clone(),
                    source,
                })?
                .path();
            if !path.is_file() {
                continue;
            }
            let text = fs::read_to_string(&path).map_err(|source| DiagnosticsError::Io {
                path: path.clone(),
                source,
            })?;
            let target = bundle_logs.join(path.file_name().expect("a listed file is named"));
            write_artifact(&target, redactor.redact_text(&text).as_ref())?;
        }
    }

    Ok(bundle)
}

/// Serves `diagnostics.export` over the core socket: assembles one
/// bundle from the real managed logs, the real managed configuration,
/// and the same health answer `health.get` serves, and reports where
/// the bundle landed. The typed query is the whole surface — no
/// broader file access rides with it.
pub struct DiagnosticsExportHandler {
    data_dir: PathBuf,
    service_version: String,
}

impl DiagnosticsExportHandler {
    /// A handler exporting from `data_dir`, answering health with the
    /// same version the core serves.
    pub fn new(data_dir: &Path, service_version: &str) -> Self {
        Self {
            data_dir: data_dir.to_path_buf(),
            service_version: service_version.to_owned(),
        }
    }
}

impl QueryHandler for DiagnosticsExportHandler {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        kanban_app::parse_payload::<DiagnosticsExportQuery>(payload)?;
        let health = serde_json::to_value(HealthResponse {
            connected: true,
            service_version: self.service_version.clone(),
        })
        .map_err(|error| ApiError::internal(&error.to_string()))?;
        let bundle = export_diagnostic_bundle(&self.data_dir, &health)
            .map_err(|error| ApiError::internal(&error.to_string()))?;
        let response = DiagnosticsExportResponse {
            bundle_dir: bundle.to_string_lossy().into_owned(),
        };
        serde_json::to_value(response).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

/// Pretty-prints one JSON artifact with a trailing newline.
fn encode_artifact(value: &Value) -> Result<String, DiagnosticsError> {
    let mut text = serde_json::to_string_pretty(value)
        .map_err(|source| DiagnosticsError::Encode { source })?;
    text.push('\n');
    Ok(text)
}

/// Writes one assembled artifact.
fn write_artifact(path: &Path, text: &str) -> Result<(), DiagnosticsError> {
    fs::write(path, text).map_err(|source| DiagnosticsError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Milliseconds since the Unix epoch, the same clock backup bundle
/// identifiers use.
fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time moves forward")
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use std::num::{NonZeroU32, NonZeroU64};
    use std::path::{Path, PathBuf};

    use serde_json::{Value, json};
    use tempfile::TempDir;

    use super::{DiagnosticsExportHandler, export_diagnostic_bundle};
    use crate::logs::{LogLevel, LogRecord, LogRotation, LogWriter};
    use crate::redaction::REDACTED;
    use crate::test_client::Client;
    use kanban_app::QueryHandler;

    const PLANTED_TOKEN: &str = "kct_t61_planted_bundle_token";
    const PLANTED_PASSPHRASE: &str = "t61-planted-bundle-passphrase";
    const PLANTED_QUOTED: &str = r#"t61 "quoted" bundle secret"#;
    const PLANTED_NON_ASCII: &str = "t61-bündlé-passphrase";

    /// Every file under `root`, recursively.
    fn walk(root: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("the directory lists") {
                let path = entry.expect("the entry reads").path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    files.push(path);
                }
            }
        }
        files.sort();
        files
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).expect("the artifact reads")
    }

    /// The secret as it appears inside one level of JSON string
    /// escaping, the form serialized log text carries.
    fn escaped(secret: &str) -> String {
        let encoded = serde_json::to_string(secret).expect("the secret serialises");
        encoded[1..encoded.len() - 1].to_owned()
    }

    /// A scratch data directory with a planted secret in its
    /// configuration, a health answer carrying the secret in free
    /// text, and one rotation of structured logs carrying the secret
    /// in a message and a field.
    fn planted_fixture() -> (TempDir, Value) {
        let dir = TempDir::new().expect("a scratch directory is available");
        std::fs::write(
            dir.path().join("config.json"),
            format!(
                r#"{{"theme":"dark","mcp_install_token":"{PLANTED_TOKEN}","backup_passphrase":"{PLANTED_PASSPHRASE}"}}"#
            ),
        )
        .expect("the configuration plants secrets");
        let writer = LogWriter::with_rotation(
            dir.path(),
            LogRotation {
                max_file_bytes: NonZeroU64::new(240).expect("the bound is not zero"),
                retained_files: NonZeroU32::new(3).expect("the count is not zero"),
            },
        )
        .expect("the log writer opens");
        for index in 0..4 {
            writer
                .append(&LogRecord::new(
                    LogLevel::Info,
                    "probe",
                    format!("bundle log entry {index} saw {PLANTED_TOKEN}"),
                ))
                .expect("the entry writes");
        }
        writer
            .append(
                &LogRecord::new(LogLevel::Info, "probe", "plain entry").with_fields(json!({
                    "note": format!("field carrying {PLANTED_PASSPHRASE}")
                })),
            )
            .expect("the field entry writes");
        let health = json!({
            "connected": true,
            "service_version": "0.1.0",
            "note": format!("installed with {PLANTED_TOKEN}"),
        });
        (dir, health)
    }

    /// KAN-T61-AC2: the bundle carries the three artifact families.
    #[test]
    fn diagnostic_bundle_exports_logs_health_and_configuration() {
        let (dir, health) = planted_fixture();

        let bundle = export_diagnostic_bundle(dir.path(), &health).expect("the bundle exports");

        assert!(
            bundle.starts_with(kanban_storage::paths::diagnostics_dir(dir.path())),
            "bundles land under the managed diagnostics directory"
        );
        let expected_health = {
            let mut text = serde_json::to_string_pretty(
                &crate::redaction::Redactor::from_config(dir.path()).redact_json(&health),
            )
            .expect("the redacted health encodes");
            text.push('\n');
            text
        };
        assert_eq!(read(&bundle.join("health.json")), expected_health);
        let config_text = read(&bundle.join("config.json"));
        assert!(config_text.contains("\"theme\": \"dark\""));
        let logs = read(&bundle.join("logs").join("core.log"));
        assert!(
            logs.contains("\"component\":\"probe\""),
            "the active log ships inside the bundle: {logs}"
        );
    }

    /// Rotation is what makes the logs bounded, so the bundle must
    /// carry every rotated file beside the active one.
    #[test]
    fn diagnostic_bundle_carries_the_rotated_log_files() {
        let (dir, health) = planted_fixture();

        let bundle = export_diagnostic_bundle(dir.path(), &health).expect("the bundle exports");

        let shipped: Vec<String> = walk(&bundle.join("logs"))
            .into_iter()
            .map(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .expect("the name is text")
                    .to_owned()
            })
            .collect();
        assert!(
            shipped.contains(&"core.log".to_owned()) && shipped.contains(&"core.log.1".to_owned()),
            "the bundle ships the active file and at least one rotated file: {shipped:?}"
        );
        assert_eq!(
            shipped.len(),
            walk(&kanban_storage::paths::logs_dir(dir.path())).len(),
            "the bundle ships exactly the files the core keeps"
        );
    }

    /// Redaction applies at assembly: the bundle is built from
    /// sources that still carry their secrets, and its artifacts come
    /// out redacted.
    #[test]
    fn diagnostic_bundle_applies_redaction_at_assembly() {
        let (dir, health) = planted_fixture();

        let bundle = export_diagnostic_bundle(dir.path(), &health).expect("the bundle exports");

        let config_text = read(&bundle.join("config.json"));
        assert!(
            config_text.contains(&format!("\"mcp_install_token\": \"{REDACTED}\"")),
            "the secret-marked configuration field collapses: {config_text}"
        );
        assert!(
            !config_text.contains(PLANTED_TOKEN) && !config_text.contains(PLANTED_PASSPHRASE),
            "no planted value survives in the exported configuration"
        );
        let health_text = read(&bundle.join("health.json"));
        assert!(
            health_text.contains(REDACTED) && !health_text.contains(PLANTED_TOKEN),
            "a secret inside a free-text health string is scrubbed: {health_text}"
        );
        for path in walk(&bundle.join("logs")) {
            let text = read(&path);
            assert!(
                !text.contains(PLANTED_TOKEN) && !text.contains(PLANTED_PASSPHRASE),
                "the log artifact {text} must ship redacted"
            );
        }
    }

    /// The KAN-T57-style guarantee, planted across both paths: no
    /// file anywhere in the exported tree carries a secret.
    #[test]
    fn secret_exclusion_holds_across_every_diagnostic_bundle_artifact() {
        let (dir, health) = planted_fixture();

        let bundle = export_diagnostic_bundle(dir.path(), &health).expect("the bundle exports");

        let artifacts = walk(&bundle);
        assert!(
            artifacts.len() >= 4,
            "the fixture ships configuration, health, and at least two log files"
        );
        for path in &artifacts {
            let text = read(path);
            assert!(
                !text.contains(PLANTED_TOKEN),
                "the planted token leaked into {}: {text}",
                path.display()
            );
            assert!(
                !text.contains(PLANTED_PASSPHRASE),
                "the planted passphrase leaked into {}: {text}",
                path.display()
            );
        }
    }

    /// The live managed logs and configuration stay untouched: the
    /// bundle is a copy, never a mutation of the sources.
    #[test]
    fn diagnostic_bundle_never_mutates_the_managed_sources() {
        let (dir, health) = planted_fixture();
        let sources = |root: &Path| -> Vec<(PathBuf, String)> {
            let source_config = root.join("config.json");
            walk(&kanban_storage::paths::logs_dir(root))
                .into_iter()
                .chain([source_config])
                .map(|path| {
                    let text = read(&path);
                    (path, text)
                })
                .collect()
        };
        let before = sources(dir.path());

        export_diagnostic_bundle(dir.path(), &health).expect("the bundle exports");

        let after = sources(dir.path());
        assert_eq!(before, after, "export is read-only over its sources");
    }

    /// A data directory with no configuration and no logs still
    /// exports a complete, honest bundle.
    #[test]
    fn diagnostic_bundle_exports_from_an_empty_data_directory() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let health = json!({ "connected": true, "service_version": "0.1.0" });

        let bundle = export_diagnostic_bundle(dir.path(), &health).expect("the bundle exports");

        let expected_health = {
            let mut text = serde_json::to_string_pretty(&health).expect("the health encodes");
            text.push('\n');
            text
        };
        assert_eq!(read(&bundle.join("health.json")), expected_health);
        assert!(
            read(&bundle.join("config.json")).contains("\"present\": false"),
            "an absent configuration is reported, not fabricated"
        );
        assert!(
            !bundle.join("logs").exists(),
            "no logs directory exists to ship"
        );
    }

    /// Fail closed: a configuration that cannot be read or parsed
    /// refuses the export before anything is created, because a
    /// bundle assembled without trustworthy redaction is unsafe data
    /// leaving the machine.
    #[test]
    fn diagnostic_bundle_fails_closed_when_the_configuration_cannot_feed_redaction() {
        let health = json!({ "connected": true, "service_version": "0.1.0" });
        let refusals = [
            ("a malformed configuration", "{ not json".to_owned()),
            ("an unreadable configuration", String::new()),
        ];
        for (label, malformed) in refusals {
            let dir = TempDir::new().expect("a scratch directory is available");
            let config_path = dir.path().join("config.json");
            if malformed.is_empty() {
                std::fs::create_dir(&config_path).expect("the unreadable configuration is planted");
            } else {
                std::fs::write(&config_path, &malformed)
                    .expect("the malformed configuration is written");
            }

            let refusal = export_diagnostic_bundle(dir.path(), &health)
                .expect_err("an unusable configuration must refuse the export");

            assert!(
                matches!(refusal, super::DiagnosticsError::Redaction { .. }),
                "{label} must refuse through redaction: {refusal}"
            );
            assert!(
                !kanban_storage::paths::diagnostics_dir(dir.path()).exists(),
                "{label} must stop the export before anything is created"
            );
        }
    }

    /// The bundle ships serialized log text, where a secret holding
    /// quotes or non-ASCII never appears as its raw bytes: the
    /// escaped, the twice-escaped, and the `\u`-escaped forms must
    /// scrub from every copied file, including ones the redacting
    /// writer never wrote.
    #[test]
    fn diagnostic_bundle_scrubs_serialized_secret_forms_from_the_shipped_logs() {
        let dir = TempDir::new().expect("a scratch directory is available");
        std::fs::write(
            dir.path().join("config.json"),
            format!(
                r#"{{"quoted_secret":"{escaped_quoted}","non_ascii_passphrase":"{PLANTED_NON_ASCII}"}}"#,
                escaped_quoted = escaped(PLANTED_QUOTED),
            ),
        )
        .expect("the configuration plants the escaping secrets");
        // A file written by anything other than the redacting writer:
        // one line carrying the single-escaped forms, one carrying the
        // twice-escaped form, one swapping the non-ASCII secret for
        // its code-point escapes.
        let once = serde_json::to_string(&json!({
            "note": format!("held {PLANTED_QUOTED} and {PLANTED_NON_ASCII}")
        }))
        .expect("the line serialises");
        let twice = serde_json::to_string(&once).expect("the line serialises again");
        let escaped_non_ascii = "t61-b\\u00fcndl\\u00e9-passphrase";
        let legacy = format!(
            "{once}\n{twice}\n{}\n",
            once.replace(PLANTED_NON_ASCII, escaped_non_ascii)
        );
        let logs = kanban_storage::paths::logs_dir(dir.path());
        std::fs::create_dir_all(&logs).expect("the logs directory is created");
        std::fs::write(logs.join("legacy.log"), legacy).expect("the legacy log is written");
        let health = json!({ "connected": true, "service_version": "0.1.0" });

        let bundle = export_diagnostic_bundle(dir.path(), &health).expect("the bundle exports");

        let shipped = read(&bundle.join("logs").join("legacy.log"));
        for form in [
            PLANTED_QUOTED.to_owned(),
            escaped(PLANTED_QUOTED),
            PLANTED_NON_ASCII.to_owned(),
            escaped_non_ascii.to_owned(),
        ] {
            assert!(
                !shipped.contains(&form),
                "every serialized form must vanish from the shipped logs: {shipped}"
            );
        }
        assert!(shipped.contains(REDACTED), "the scrub leaves its marker");
    }

    /// The typed handler is the narrow boundary: it rejects unknown
    /// fields like every catalogued query and answers with exactly the
    /// bundle directory.
    #[test]
    fn the_typed_export_handler_keeps_the_query_surface_narrow() {
        let (dir, _health) = planted_fixture();
        let handler = DiagnosticsExportHandler::new(dir.path(), "0.1.0-test");

        let refusal = handler
            .handle(&json!({ "surprise": 1 }))
            .expect_err("unknown fields are rejected");
        assert_eq!(
            refusal.code,
            kanban_dto::ErrorCode::UnknownField,
            "the typed payload keeps its closed shape"
        );

        let answer = handler.handle(&json!({})).expect("the export answers");
        let bundle_dir = answer["bundle_dir"].as_str().expect("the answer is text");
        assert!(
            Path::new(bundle_dir).starts_with(kanban_storage::paths::diagnostics_dir(dir.path())),
            "the answer names the exported bundle: {bundle_dir}"
        );
        assert_eq!(
            answer.as_object().expect("the answer is an object").len(),
            1,
            "the answer carries exactly the bundle directory"
        );
    }

    /// KAN-T61-AC2 wiring: the real boundary is the served socket. The
    /// production core exports a redacted bundle from its real
    /// configuration, its real logs, and the same health answer
    /// `health.get` serves — reached through the typed query alone.
    #[test]
    fn diagnostics_export_serves_a_redacted_bundle_over_the_socket() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let nested_secret = "hct_live_t61_nested_under_credentials";
        std::fs::write(
            dir.path().join("config.json"),
            format!(
                r#"{{"theme":"dark","mcp_install_token":"{PLANTED_TOKEN}","credentials":{{"herdr":"{nested_secret}"}}}}"#
            ),
        )
        .expect("the configuration plants secrets at both depths");
        let writer = LogWriter::with_rotation(
            dir.path(),
            LogRotation {
                max_file_bytes: NonZeroU64::new(240).expect("the bound is not zero"),
                retained_files: NonZeroU32::new(3).expect("the count is not zero"),
            },
        )
        .expect("the log writer opens");
        for index in 0..4 {
            writer
                .append(&LogRecord::new(
                    LogLevel::Info,
                    "probe",
                    format!("socket bundle entry {index} saw {PLANTED_TOKEN} and {nested_secret}"),
                ))
                .expect("the entry writes");
        }
        let core = crate::test_client::boot(&dir);
        let mut client = Client::connect(core.socket_path());

        let health = client.query("health.get");
        let answer = client.query("diagnostics.export");
        let bundle_dir = answer["bundle_dir"].as_str().expect("the answer is text");
        let bundle = Path::new(bundle_dir);
        assert!(
            bundle.starts_with(kanban_storage::paths::diagnostics_dir(dir.path())),
            "the served answer names a bundle under managed diagnostics: {bundle_dir}"
        );

        // The health artifact is the real health answer, not a
        // hand-built stand-in.
        let shipped_health: Value =
            serde_json::from_str(&read(&bundle.join("health.json"))).expect("health parses");
        assert_eq!(shipped_health, health, "the bundle ships the served health");

        // The whole exported tree stays free of every planted secret,
        // the nested one included, across active and rotated logs.
        let artifacts = walk(bundle);
        assert!(
            artifacts.len() >= 4,
            "the fixture ships configuration, health, and at least two log files"
        );
        for path in &artifacts {
            let text = read(path);
            for secret in [PLANTED_TOKEN, nested_secret] {
                assert!(
                    !text.contains(secret),
                    "the planted secret `{secret}` leaked into {}: {text}",
                    path.display()
                );
            }
        }
        let config_text = read(&bundle.join("config.json"));
        assert!(
            config_text.contains("\"credentials\": \"[REDACTED]\""),
            "the nested secret-marked container collapses: {config_text}"
        );
        assert!(
            walk(&bundle.join("logs"))
                .iter()
                .any(|path| path.ends_with("core.log.1")),
            "the bundle ships the rotated log beside the active one"
        );

        // The typed boundary stays closed: an unknown field is refused.
        let refused = client.attempt("query", "diagnostics.export", json!({ "surprise": 1 }));
        assert_eq!(
            refused["kind"], "error",
            "unknown fields are refused: {refused}"
        );
        assert_eq!(refused["error"]["code"], "unknown_field");

        core.shutdown();
    }

    /// Fail closed over the boundary too: while the core keeps serving
    /// (health still answers) a configuration that cannot feed the
    /// redactor makes the export refuse, and nothing is created.
    #[test]
    fn diagnostics_export_refuses_over_the_socket_when_redaction_cannot_be_trusted() {
        let dir = TempDir::new().expect("a scratch directory is available");
        std::fs::write(dir.path().join("config.json"), "{ not json")
            .expect("the malformed configuration is written");
        let core = crate::test_client::boot(&dir);
        let mut client = Client::connect(core.socket_path());

        assert_eq!(
            client.query("health.get")["connected"],
            json!(true),
            "serving survives the unusable configuration"
        );
        let refused = client.attempt("query", "diagnostics.export", json!({}));
        assert_eq!(refused["kind"], "error", "the export refuses: {refused}");
        assert_eq!(
            refused["error"]["code"], "internal",
            "the refusal reports itself as an export failure"
        );
        assert!(
            !kanban_storage::paths::diagnostics_dir(dir.path()).exists(),
            "no bundle may be created from an unredactable data directory"
        );

        core.shutdown();
    }
}
