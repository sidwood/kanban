//! Structured, size-bounded rotating logs under managed application
//! data (DR-RB-11).
//!
//! Every entry is one JSON line in `logs/core.log`. When the active
//! file would pass its byte bound, rotation shifts it aside and keeps
//! a fixed number of files, so the directory's total size stays
//! bounded no matter how long the core runs. Entries are redacted as
//! they are written, and an append that fails never stops the core:
//! callers ignore append results by design, because diagnostics must
//! never outrank serving.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::num::{NonZeroU32, NonZeroU64};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use kanban_storage::paths::logs_dir;

use crate::redaction::Redactor;

/// The active log file inside the logs directory.
const ACTIVE_FILE: &str = "core.log";

/// How large one file may grow and how many files rotation keeps, the
/// active file included.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogRotation {
    /// The byte bound a file passes to rotate.
    pub max_file_bytes: NonZeroU64,
    /// How many files exist at once, the active file included.
    pub retained_files: NonZeroU32,
}

impl LogRotation {
    /// The product default: one-mebibyte files, five kept.
    pub const PRODUCT: Self = Self {
        max_file_bytes: match NonZeroU64::new(1024 * 1024) {
            Some(bytes) => bytes,
            None => panic!("one mebibyte is not zero"),
        },
        retained_files: match NonZeroU32::new(5) {
            Some(files) => files,
            None => panic!("five is not zero"),
        },
    };
}

/// One structured log entry's severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    /// Routine operation.
    Info,
    /// Something the Operator should notice.
    Warn,
    /// Something that failed.
    Error,
}

impl LogLevel {
    fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

/// One structured log entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogRecord {
    /// The entry's severity.
    pub level: LogLevel,
    /// Which part of the core wrote the entry.
    pub component: String,
    /// What happened, in one line.
    pub message: String,
    /// Extra structured facts; always a JSON object.
    pub fields: Value,
}

impl LogRecord {
    /// An entry carrying no extra facts.
    pub fn new(level: LogLevel, component: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            level,
            component: component.into(),
            message: message.into(),
            fields: json!({}),
        }
    }

    /// Attach extra structured facts to the entry.
    pub fn with_fields(mut self, fields: Value) -> Self {
        self.fields = fields;
        self
    }
}

/// Writes structured entries to the managed logs directory, rotating
/// with bounded size and redacting configured secrets at write.
#[derive(Debug)]
pub struct LogWriter {
    directory: PathBuf,
    rotation: LogRotation,
    redactor: Redactor,
    append_gate: Mutex<()>,
}

impl LogWriter {
    /// Opens the managed logs directory with the product rotation and
    /// the secrets planted in the managed configuration.
    pub fn open(data_dir: &Path) -> std::io::Result<Self> {
        Self::with_rotation(data_dir, LogRotation::PRODUCT)
    }

    /// Opens the managed logs directory under an explicit rotation.
    pub fn with_rotation(data_dir: &Path, rotation: LogRotation) -> std::io::Result<Self> {
        let directory = logs_dir(data_dir);
        fs::create_dir_all(&directory)?;
        Ok(Self {
            directory,
            rotation,
            redactor: Redactor::from_config(data_dir),
            append_gate: Mutex::new(()),
        })
    }

    /// The directory this writer rotates inside.
    pub fn directory(&self) -> &Path {
        self.directory.as_path()
    }

    /// Appends one entry as a redacted JSON line, rotating first when
    /// the line would pass the active file's bound.
    pub fn append(&self, record: &LogRecord) -> std::io::Result<()> {
        let entry = json!({
            "ts": unix_millis(),
            "level": record.level.as_str(),
            "component": record.component,
            "message": self.redactor.redact_text(&record.message),
            "fields": self.redactor.redact_json(&record.fields),
        });
        let mut line = serde_json::to_string(&entry).map_err(std::io::Error::other)?;
        line.push('\n');

        // One writer at a time holds the gate, so the size check, the
        // rotation, and the append act on the same directory state.
        let _gate = self
            .append_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let active = self.directory.join(ACTIVE_FILE);
        let size = fs::metadata(&active).map(|meta| meta.len()).unwrap_or(0);
        if size > 0 && size + line.len() as u64 > self.rotation.max_file_bytes.get() {
            rotate(&self.directory, self.rotation.retained_files.get())?;
        }
        let mut file = OpenOptions::new().create(true).append(true).open(&active)?;
        file.write_all(line.as_bytes())
    }
}

/// The path of the rotated file at `index`, where 1 is the most
/// recently retired active file.
fn rotated_path(directory: &Path, index: u32) -> PathBuf {
    directory.join(format!("{ACTIVE_FILE}.{index}"))
}

/// Shifts the active file aside, dropping the oldest file when the
/// retained count is already full. A single retained file has no
/// rotated slots, so rotation truncates the active file instead.
fn rotate(directory: &Path, retained_files: u32) -> std::io::Result<()> {
    if retained_files == 1 {
        return fs::remove_file(directory.join(ACTIVE_FILE));
    }
    let oldest = rotated_path(directory, retained_files - 1);
    if oldest.exists() {
        fs::remove_file(oldest)?;
    }
    for index in (1..retained_files - 1).rev() {
        let from = rotated_path(directory, index);
        let to = rotated_path(directory, index + 1);
        if from.exists() {
            fs::rename(from, to)?;
        }
    }
    fs::rename(directory.join(ACTIVE_FILE), rotated_path(directory, 1))
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

    use serde_json::json;
    use tempfile::TempDir;

    use super::{LogLevel, LogRecord, LogRotation, LogWriter};
    use crate::redaction::REDACTED;

    const PLANTED_SECRET: &str = "kct_t61_planted_log_secret";

    /// A writer bound to `max_file_bytes` per file and `retained_files`
    /// total, matching lines roughly this long.
    fn bounded_writer(dir: &TempDir, max_file_bytes: u64, retained_files: u32) -> LogWriter {
        let rotation = LogRotation {
            max_file_bytes: NonZeroU64::new(max_file_bytes).expect("the bound is not zero"),
            retained_files: NonZeroU32::new(retained_files).expect("the count is not zero"),
        };
        LogWriter::with_rotation(dir.path(), rotation).expect("the bounded writer opens")
    }

    fn read_lines(path: &std::path::Path) -> Vec<serde_json::Value> {
        let text = std::fs::read_to_string(path).expect("the log file reads");
        text.lines()
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_str(line).expect("every line is one JSON entry"))
            .collect()
    }

    fn log_files(dir: &TempDir) -> Vec<std::path::PathBuf> {
        let mut files: Vec<_> = std::fs::read_dir(super::logs_dir(dir.path()))
            .expect("the logs directory lists")
            .map(|entry| entry.expect("the entry reads").path())
            .filter(|path| path.is_file())
            .collect();
        files.sort();
        files
    }

    #[test]
    fn structured_log_entries_write_as_json_lines() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let writer = bounded_writer(&dir, 8192, 3);
        writer
            .append(&LogRecord::new(LogLevel::Info, "service", "core starting"))
            .expect("the first entry writes");
        writer
            .append(
                &LogRecord::new(LogLevel::Warn, "backup", "backup failed")
                    .with_fields(json!({ "attempt": 2 })),
            )
            .expect("the second entry writes");

        let entries = read_lines(&super::logs_dir(dir.path()).join("core.log"));

        assert_eq!(entries.len(), 2, "one line per entry");
        assert_eq!(entries[0]["level"], json!("info"));
        assert_eq!(entries[0]["component"], json!("service"));
        assert_eq!(entries[0]["message"], json!("core starting"));
        assert!(
            entries[0]["ts"].as_u64().is_some_and(|ts| ts > 0),
            "the entry carries its wall-clock timestamp"
        );
        assert_eq!(entries[1]["level"], json!("warn"));
        assert_eq!(entries[1]["fields"], json!({ "attempt": 2 }));
    }

    #[test]
    fn structured_logs_rotate_at_the_size_bound() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let writer = bounded_writer(&dir, 220, 3);
        for index in 0..4 {
            writer
                .append(&LogRecord::new(
                    LogLevel::Info,
                    "probe",
                    format!("rotation marker entry {index:03} of the bounded log"),
                ))
                .expect("every entry writes");
        }

        let logs = super::logs_dir(dir.path());
        let rotated = logs.join("core.log.1");
        assert!(
            rotated.is_file(),
            "passing the per-file bound must retire the active file"
        );
        for file in log_files(&dir) {
            let size = std::fs::metadata(&file).expect("the metadata reads").len();
            assert!(
                size <= 220,
                "no file may pass the bound: {} is {size} bytes",
                file.display()
            );
        }
        let active = read_lines(&logs.join("core.log"));
        assert_eq!(
            active.last().and_then(|entry| entry["message"].as_str()),
            Some("rotation marker entry 003 of the bounded log"),
            "the newest entry stays in the active file"
        );
    }

    #[test]
    fn rotated_logs_stay_within_the_bounded_total_size() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let max_file_bytes = 240_u64;
        let retained_files = 4;
        let writer = bounded_writer(&dir, max_file_bytes, retained_files);
        for index in 0..40 {
            writer
                .append(&LogRecord::new(
                    LogLevel::Info,
                    "probe",
                    format!("bounded total size marker entry {index:03} of the long log"),
                ))
                .expect("every entry writes");
        }

        let files = log_files(&dir);
        assert!(
            files.len() <= retained_files as usize,
            "rotation must cap the file count, found {}",
            files.len()
        );
        let total: u64 = files
            .iter()
            .map(|file| std::fs::metadata(file).expect("the metadata reads").len())
            .sum();
        assert!(
            total <= max_file_bytes * retained_files as u64,
            "the logs directory must stay bounded, found {total} bytes across {} files",
            files.len()
        );
        assert_eq!(
            read_lines(&super::logs_dir(dir.path()).join("core.log"))
                .last()
                .and_then(|entry| entry["message"].as_str()),
            Some("bounded total size marker entry 039 of the long log"),
            "no entry is dropped while bounding"
        );
    }

    #[test]
    fn log_rotation_preserves_entry_order() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let writer = bounded_writer(&dir, 200, 5);
        for index in 1..=20 {
            writer
                .append(&LogRecord::new(
                    LogLevel::Info,
                    "probe",
                    format!("ordered marker {index:02}"),
                ))
                .expect("every entry writes");
        }

        let mut messages = Vec::new();
        for index in (1..5).rev() {
            for entry in read_lines(&super::rotated_path(&super::logs_dir(dir.path()), index)) {
                messages.push(
                    entry["message"]
                        .as_str()
                        .expect("the message is text")
                        .to_owned(),
                );
            }
        }
        for entry in read_lines(&super::logs_dir(dir.path()).join("core.log")) {
            messages.push(
                entry["message"]
                    .as_str()
                    .expect("the message is text")
                    .to_owned(),
            );
        }

        let mut expected: Vec<String> = (1..=20)
            .map(|index| format!("ordered marker {index:02}"))
            .collect();
        let retained_suffix = expected.split_off(expected.len().saturating_sub(messages.len()));
        assert_eq!(
            messages, retained_suffix,
            "reading oldest file to active must replay exactly the retained entries in write order"
        );
    }

    #[test]
    fn secret_exclusion_redacts_configured_secrets_in_written_logs() {
        let dir = TempDir::new().expect("a scratch directory is available");
        std::fs::write(
            dir.path().join("config.json"),
            format!(r#"{{"mcp_install_token":"{PLANTED_SECRET}"}}"#),
        )
        .expect("the configuration plants a secret");
        let writer = bounded_writer(&dir, 260, 2);
        for index in 0..3 {
            writer
                .append(&LogRecord::new(
                    LogLevel::Info,
                    "probe",
                    format!("entry {index} saw {PLANTED_SECRET} pass by"),
                ))
                .expect("every entry writes");
            writer
                .append(
                    &LogRecord::new(LogLevel::Info, "probe", "plain entry").with_fields(json!({
                        "note": format!("field carrying {PLANTED_SECRET}")
                    })),
                )
                .expect("every field entry writes");
        }

        for file in log_files(&dir) {
            let text = std::fs::read_to_string(&file).expect("the log file reads");
            assert!(
                !text.contains(PLANTED_SECRET),
                "the planted secret must never land in {}: {text}",
                file.display()
            );
            assert!(
                text.contains(REDACTED),
                "redaction replaces the secret in {}",
                file.display()
            );
        }
    }

    /// KAN-T61-AC1 wiring: the production `serve` path owns the logs
    /// directory and writes real entries — its own startup, and the
    /// backup outcomes the scheduler records.
    #[test]
    fn the_running_core_writes_structured_logs() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let core = crate::test_client::boot(&dir);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if dir.path().join(".backup-scheduler.json").is_file() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the initial scheduled backup settles before the log is read"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        core.shutdown();

        let text = std::fs::read_to_string(super::logs_dir(dir.path()).join("core.log"))
            .expect("the core log reads");
        assert!(
            text.contains("\"component\":\"service\""),
            "the core's own lifecycle lands in the log: {text}"
        );
        assert!(
            text.contains("\"component\":\"backup\""),
            "scheduled backup outcomes land in the log: {text}"
        );
    }
}
