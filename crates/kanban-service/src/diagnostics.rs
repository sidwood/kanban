//! The exportable diagnostic bundle: logs, health, and configuration
//! assembled under managed application data with redaction applied at
//! assembly (KAN-S13-US4, DR-RB-11, DR-SS-15).
//!
//! Export copies the structured logs beside a snapshot of the current
//! health answer and the managed configuration. Nothing carries a
//! secret out of the bundle: the redactor built from the managed
//! configuration scrubs every artifact as it is written, so a secret
//! planted anywhere the core could echo it still never appears in
//! the exported tree.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use kanban_storage::paths::{config_file_name, diagnostics_dir, logs_dir};

use crate::redaction::Redactor;

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
}

/// Exports one diagnostic bundle under `diagnostics/<bundle id>/`
/// holding the redacted logs, the redacted health snapshot, and the
/// redacted managed configuration, and returns the bundle directory.
pub fn export_diagnostic_bundle(
    data_dir: &Path,
    health: &Value,
) -> Result<PathBuf, DiagnosticsError> {
    let redactor = Redactor::from_config(data_dir);
    let bundle = diagnostics_dir(data_dir).join(unix_millis().to_string());
    fs::create_dir_all(&bundle).map_err(|source| DiagnosticsError::Io {
        path: bundle.clone(),
        source,
    })?;

    write_artifact(
        &bundle.join("health.json"),
        &encode_artifact(&redactor.redact_json(health))?,
    )?;

    let configuration = fs::read_to_string(data_dir.join(config_file_name()))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
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

    use super::export_diagnostic_bundle;
    use crate::logs::{LogLevel, LogRecord, LogRotation, LogWriter};
    use crate::redaction::REDACTED;

    const PLANTED_TOKEN: &str = "kct_t61_planted_bundle_token";
    const PLANTED_PASSPHRASE: &str = "t61-planted-bundle-passphrase";

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
}
