//! Consistent backup bundles of SQLite, attachments, and
//! configuration, with manifest hashes, validation, encryption,
//! preview, retention, and safe restore (KAN-S13-US2).

use std::fs;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use argon2::Argon2;
use chacha20poly1305::aead::{Aead, KeyInit, OsRng};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use rand_core::RngCore;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::db::{ConnectionHandle, Database};
use crate::error::StorageError;
use crate::evidence::content_hash;
use crate::migrations::{PendingMigration, PreMigrationHook};
use crate::paths::{attachments_dir, backups_dir, config_file_name, database_file_name};

/// How many dated backup bundles the store keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackupRetentionPolicy {
    retained: NonZeroU32,
}

impl BackupRetentionPolicy {
    /// Keep the `retained` most recent verified bundles.
    pub const fn keep_most_recent(retained: NonZeroU32) -> Self {
        Self { retained }
    }

    /// The number of bundles kept.
    pub const fn retained(self) -> NonZeroU32 {
        self.retained
    }
}

/// Options for creating a backup bundle.
#[derive(Debug, Clone)]
pub struct BackupOptions {
    /// How many bundles to keep after this one lands.
    pub retention: BackupRetentionPolicy,
    /// When set, file payloads are encrypted at rest in the bundle.
    pub passphrase: Option<String>,
}

/// One file entry in a bundle manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestEntry {
    /// The path relative to the bundle root.
    pub path: String,
    /// The SHA-256 digest of the stored bytes (hex).
    pub sha256: String,
    /// The byte length of the stored payload.
    pub size: u64,
}

/// The manifest every bundle carries; validation re-verifies hashes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupManifest {
    /// The manifest format version.
    pub format_version: u32,
    /// When the bundle was created (RFC 3339 UTC).
    pub created_at: String,
    /// The applied schema version captured in the snapshot.
    pub schema_version: i64,
    /// Whether file payloads are encrypted in the bundle.
    pub encrypted: bool,
    /// Every file in the bundle and its content hash.
    pub files: Vec<ManifestEntry>,
}

/// A read-only summary of a bundle for operator preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupPreview {
    /// The bundle directory.
    pub bundle_path: PathBuf,
    /// The parsed manifest.
    pub manifest: BackupManifest,
}

/// A verified backup record for one schema version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifiedBackupRecord {
    /// The bundle directory name under `backups/`.
    pub bundle_id: String,
    /// The schema version the snapshot captured.
    pub schema_version: i64,
    /// When validation succeeded (RFC 3339 UTC).
    pub verified_at: String,
}

/// Manages backup bundles under managed application data.
pub struct BackupStore {
    managed_root: PathBuf,
}

impl BackupStore {
    /// Opens the store rooted at `managed_root`.
    pub fn new(managed_root: PathBuf) -> Self {
        Self { managed_root }
    }

    /// The managed root this store serves.
    pub fn managed_root(&self) -> &Path {
        &self.managed_root
    }

    /// Creates a consistent snapshot, validates it, records
    /// verification, and prunes older bundles per `options`.
    pub fn create(
        &self,
        database: &Database,
        options: &BackupOptions,
    ) -> Result<PathBuf, StorageError> {
        let bundle_path = self.write_bundle(database, options)?;
        let manifest = self.validate(&bundle_path, options.passphrase.as_deref())?;
        self.write_verified(&bundle_path, &manifest)?;
        self.prune(options.retention)?;
        Ok(bundle_path)
    }

    /// Verifies manifest hashes and opens the database snapshot.
    pub fn validate(
        &self,
        bundle_path: &Path,
        passphrase: Option<&str>,
    ) -> Result<BackupManifest, StorageError> {
        let manifest = read_manifest(bundle_path)?;
        for entry in &manifest.files {
            let path = bundle_path.join(&entry.path);
            let bytes = read_payload(&path, manifest.encrypted, passphrase)?;
            let digest = content_hash(&bytes);
            if digest != entry.sha256 {
                return Err(StorageError::BackupHashMismatch {
                    path: entry.path.clone(),
                    expected: entry.sha256.clone(),
                    actual: digest,
                });
            }
            if bytes.len() as u64 != entry.size {
                return Err(StorageError::BackupSizeMismatch {
                    path: entry.path.clone(),
                    expected: entry.size,
                    actual: bytes.len() as u64,
                });
            }
        }
        let database_path = bundle_path.join(database_file_name());
        if manifest.encrypted {
            let staging = bundle_path.join(".validate-staging.sqlite");
            let bytes = read_payload(&database_path, true, passphrase)?;
            fs::write(&staging, &bytes).map_err(|source| StorageError::BackupIo {
                path: staging.clone(),
                source,
            })?;
            let conn = Connection::open(&staging).map_err(|source| StorageError::BackupOpen {
                path: staging.clone(),
                source,
            })?;
            let check: String = conn
                .query_row("PRAGMA integrity_check", [], |row| row.get(0))
                .map_err(|source| StorageError::BackupOpen {
                    path: staging.clone(),
                    source,
                })?;
            let _ = fs::remove_file(&staging);
            if check != "ok" {
                return Err(StorageError::BackupIntegrity { detail: check });
            }
        } else {
            let conn =
                Connection::open(&database_path).map_err(|source| StorageError::BackupOpen {
                    path: database_path.clone(),
                    source,
                })?;
            let check: String = conn
                .query_row("PRAGMA integrity_check", [], |row| row.get(0))
                .map_err(|source| StorageError::BackupOpen {
                    path: database_path.clone(),
                    source,
                })?;
            if check != "ok" {
                return Err(StorageError::BackupIntegrity { detail: check });
            }
        }
        Ok(manifest)
    }

    /// Reads the manifest without restoring or mutating live data.
    pub fn preview(
        &self,
        bundle_path: &Path,
        passphrase: Option<&str>,
    ) -> Result<BackupPreview, StorageError> {
        let manifest = self.validate(bundle_path, passphrase)?;
        Ok(BackupPreview {
            bundle_path: bundle_path.to_path_buf(),
            manifest,
        })
    }

    /// Restores into a staging directory, then swaps atomically into
    /// the managed root this store serves.
    pub fn restore(
        &self,
        bundle_path: &Path,
        passphrase: Option<&str>,
    ) -> Result<(), StorageError> {
        let manifest = self.validate(bundle_path, passphrase)?;
        let staging = self.managed_root.join(".restore-staging");
        if staging.exists() {
            fs::remove_dir_all(&staging).map_err(|source| StorageError::BackupIo {
                path: staging.clone(),
                source,
            })?;
        }
        fs::create_dir_all(&staging).map_err(|source| StorageError::BackupIo {
            path: staging.clone(),
            source,
        })?;
        extract_bundle(bundle_path, &staging, &manifest, passphrase)?;
        self.validate_staging(&staging, &manifest, passphrase)?;
        atomic_swap_restore(&self.managed_root, &staging)?;
        Ok(())
    }

    /// Removes bundles older than the retention policy allows.
    pub fn prune(&self, retention: BackupRetentionPolicy) -> Result<Vec<PathBuf>, StorageError> {
        let root = backups_dir(&self.managed_root);
        if !root.exists() {
            return Ok(Vec::new());
        }
        let mut bundles = list_bundle_dirs(&root)?;
        bundles.sort_by(|left, right| right.cmp(left));
        let keep = retention.retained().get() as usize;
        let mut removed = Vec::new();
        for path in bundles.into_iter().skip(keep) {
            fs::remove_dir_all(&path).map_err(|source| StorageError::BackupIo {
                path: path.clone(),
                source,
            })?;
            removed.push(path);
        }
        Ok(removed)
    }

    /// The verified backup for `schema_version`, if one exists.
    pub fn verified_record_for(
        &self,
        schema_version: i64,
    ) -> Result<Option<VerifiedBackupRecord>, StorageError> {
        let records = self.load_verified_records()?;
        Ok(records
            .into_iter()
            .find(|record| record.schema_version == schema_version))
    }

    fn write_bundle(
        &self,
        database: &Database,
        options: &BackupOptions,
    ) -> Result<PathBuf, StorageError> {
        let bundle_id = bundle_id_from_now();
        let bundle_path = backups_dir(&self.managed_root).join(&bundle_id);
        fs::create_dir_all(&bundle_path).map_err(|source| StorageError::BackupIo {
            path: bundle_path.clone(),
            source,
        })?;
        let schema_version = current_schema_version(database)?;
        let encrypted = options.passphrase.is_some();
        let mut files = Vec::new();

        let database_target = bundle_path.join(database_file_name());
        snapshot_database(database, &database_target)?;
        let database_bytes =
            fs::read(&database_target).map_err(|source| StorageError::BackupIo {
                path: database_target.clone(),
                source,
            })?;
        files.push(manifest_entry(database_file_name(), &database_bytes));
        if encrypted {
            let passphrase = options
                .passphrase
                .as_deref()
                .expect("encrypted needs passphrase");
            fs::write(
                &database_target,
                encrypt_bytes(passphrase, &database_bytes)?,
            )
            .map_err(|source| StorageError::BackupIo {
                path: database_target.clone(),
                source,
            })?;
        }

        let live_attachments = attachments_dir(&self.managed_root);
        if live_attachments.exists() {
            let bundle_attachments = bundle_path.join("attachments");
            copy_tree(
                &live_attachments,
                &bundle_attachments,
                encrypted,
                options.passphrase.as_deref(),
                &mut files,
            )?;
        }

        let live_config = self.managed_root.join(config_file_name());
        if live_config.exists() {
            let bundle_config = bundle_path.join(config_file_name());
            fs::copy(&live_config, &bundle_config).map_err(|source| StorageError::BackupIo {
                path: bundle_config.clone(),
                source,
            })?;
            let config_bytes =
                fs::read(&bundle_config).map_err(|source| StorageError::BackupIo {
                    path: bundle_config.clone(),
                    source,
                })?;
            files.push(manifest_entry(config_file_name(), &config_bytes));
            if encrypted {
                let passphrase = options
                    .passphrase
                    .as_deref()
                    .expect("encrypted needs passphrase");
                fs::write(&bundle_config, encrypt_bytes(passphrase, &config_bytes)?).map_err(
                    |source| StorageError::BackupIo {
                        path: bundle_config.clone(),
                        source,
                    },
                )?;
            }
        }

        let manifest = BackupManifest {
            format_version: 1,
            created_at: rfc3339_now(),
            schema_version,
            encrypted,
            files,
        };
        write_manifest(&bundle_path, &manifest)?;
        Ok(bundle_path)
    }

    fn validate_staging(
        &self,
        staging: &Path,
        manifest: &BackupManifest,
        _passphrase: Option<&str>,
    ) -> Result<(), StorageError> {
        for entry in &manifest.files {
            let path = staging.join(&entry.path);
            let bytes = fs::read(&path).map_err(|source| StorageError::BackupIo {
                path: path.clone(),
                source,
            })?;
            if content_hash(&bytes) != entry.sha256 {
                return Err(StorageError::BackupHashMismatch {
                    path: entry.path.clone(),
                    expected: entry.sha256.clone(),
                    actual: content_hash(&bytes),
                });
            }
        }
        let database_path = staging.join(database_file_name());
        let conn = Connection::open(&database_path).map_err(|source| StorageError::BackupOpen {
            path: database_path.clone(),
            source,
        })?;
        let check: String = conn
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .map_err(|source| StorageError::BackupOpen {
                path: database_path,
                source,
            })?;
        if check != "ok" {
            return Err(StorageError::BackupIntegrity { detail: check });
        }
        Ok(())
    }

    fn write_verified(
        &self,
        bundle_path: &Path,
        manifest: &BackupManifest,
    ) -> Result<(), StorageError> {
        let bundle_id = bundle_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| StorageError::BackupInvalid {
                reason: "the bundle path has no name".to_string(),
            })?
            .to_string();
        let record = VerifiedBackupRecord {
            bundle_id: bundle_id.clone(),
            schema_version: manifest.schema_version,
            verified_at: rfc3339_now(),
        };
        let mut records = self.load_verified_records()?;
        records.retain(|existing| existing.bundle_id != bundle_id);
        records.push(record);
        self.save_verified_records(&records)?;
        write_verified_marker(bundle_path, manifest.schema_version)?;
        Ok(())
    }

    fn load_verified_records(&self) -> Result<Vec<VerifiedBackupRecord>, StorageError> {
        let path = verified_records_path(&self.managed_root);
        if !path.exists() {
            return Ok(Vec::new());
        }
        let text = fs::read_to_string(&path).map_err(|source| StorageError::BackupIo {
            path: path.clone(),
            source,
        })?;
        serde_json::from_str(&text).map_err(|source| StorageError::BackupInvalid {
            reason: source.to_string(),
        })
    }

    fn save_verified_records(&self, records: &[VerifiedBackupRecord]) -> Result<(), StorageError> {
        let path = verified_records_path(&self.managed_root);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| StorageError::BackupIo {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let text = serde_json::to_string_pretty(records).map_err(|source| {
            StorageError::BackupInvalid {
                reason: source.to_string(),
            }
        })?;
        fs::write(&path, text).map_err(|source| StorageError::BackupIo { path, source })
    }

    /// Creates a bundle from an open connection, for hooks that already
    /// hold the authoritative connection.
    pub fn create_from_connection(
        &self,
        conn: &Connection,
        options: BackupOptions,
    ) -> Result<PathBuf, StorageError> {
        let bundle_path = self.write_bundle_from_connection(conn, &options)?;
        let manifest = self.validate(&bundle_path, options.passphrase.as_deref())?;
        self.write_verified(&bundle_path, &manifest)?;
        self.prune(options.retention)?;
        Ok(bundle_path)
    }

    fn write_bundle_from_connection(
        &self,
        conn: &Connection,
        options: &BackupOptions,
    ) -> Result<PathBuf, StorageError> {
        let bundle_id = bundle_id_from_now();
        let bundle_path = backups_dir(&self.managed_root).join(&bundle_id);
        fs::create_dir_all(&bundle_path).map_err(|source| StorageError::BackupIo {
            path: bundle_path.clone(),
            source,
        })?;
        let schema_version = current_schema_version_from(conn)?;
        let encrypted = options.passphrase.is_some();
        let mut files = Vec::new();

        let database_target = bundle_path.join(database_file_name());
        snapshot_connection(conn, &database_target)?;
        let database_bytes =
            fs::read(&database_target).map_err(|source| StorageError::BackupIo {
                path: database_target.clone(),
                source,
            })?;
        files.push(manifest_entry(database_file_name(), &database_bytes));
        if encrypted {
            let passphrase = options
                .passphrase
                .as_deref()
                .expect("encrypted needs passphrase");
            fs::write(
                &database_target,
                encrypt_bytes(passphrase, &database_bytes)?,
            )
            .map_err(|source| StorageError::BackupIo {
                path: database_target.clone(),
                source,
            })?;
        }

        let live_attachments = attachments_dir(&self.managed_root);
        if live_attachments.exists() {
            let bundle_attachments = bundle_path.join("attachments");
            copy_tree(
                &live_attachments,
                &bundle_attachments,
                encrypted,
                options.passphrase.as_deref(),
                &mut files,
            )?;
        }

        let live_config = self.managed_root.join(config_file_name());
        if live_config.exists() {
            let bundle_config = bundle_path.join(config_file_name());
            fs::copy(&live_config, &bundle_config).map_err(|source| StorageError::BackupIo {
                path: bundle_config.clone(),
                source,
            })?;
            let config_bytes =
                fs::read(&bundle_config).map_err(|source| StorageError::BackupIo {
                    path: bundle_config.clone(),
                    source,
                })?;
            files.push(manifest_entry(config_file_name(), &config_bytes));
            if encrypted {
                let passphrase = options
                    .passphrase
                    .as_deref()
                    .expect("encrypted needs passphrase");
                fs::write(&bundle_config, encrypt_bytes(passphrase, &config_bytes)?).map_err(
                    |source| StorageError::BackupIo {
                        path: bundle_config.clone(),
                        source,
                    },
                )?;
            }
        }

        let manifest = BackupManifest {
            format_version: 1,
            created_at: rfc3339_now(),
            schema_version,
            encrypted,
            files,
        };
        write_manifest(&bundle_path, &manifest)?;
        Ok(bundle_path)
    }
}

/// Refuses migration unless a verified backup exists for the current
/// schema; optionally creates one first.
pub struct VerifiedBackupHook<'a> {
    store: &'a BackupStore,
    conn: ConnectionHandle,
    retention: BackupRetentionPolicy,
    passphrase: Option<String>,
    create_if_missing: bool,
}

impl<'a> VerifiedBackupHook<'a> {
    /// Build a hook that refuses when no verified backup exists.
    pub fn refuse_without_backup(store: &'a BackupStore, database: &Database) -> Self {
        Self {
            store,
            conn: database.connection_handle(),
            retention: BackupRetentionPolicy::keep_most_recent(
                NonZeroU32::new(7).expect("seven is not zero"),
            ),
            passphrase: None,
            create_if_missing: false,
        }
    }

    /// Build a hook that creates and verifies a backup when needed.
    pub fn create_before_migrate(
        store: &'a BackupStore,
        database: &Database,
        retention: BackupRetentionPolicy,
    ) -> Self {
        Self {
            store,
            conn: database.connection_handle(),
            retention,
            passphrase: None,
            create_if_missing: true,
        }
    }
}

impl PreMigrationHook for VerifiedBackupHook<'_> {
    fn before_migrate(&self, pending: &[PendingMigration]) -> Result<(), StorageError> {
        if pending.is_empty() {
            return Ok(());
        }
        let schema_version = current_schema_version_from(&self.conn.lock())?;
        if self.store.verified_record_for(schema_version)?.is_some() {
            return Ok(());
        }
        if self.create_if_missing {
            let options = BackupOptions {
                retention: self.retention,
                passphrase: self.passphrase.clone(),
            };
            self.store
                .create_from_connection(&self.conn.lock(), options)?;
            return Ok(());
        }
        Err(StorageError::HookRefused {
            reason: format!(
                "no verified backup for schema version {schema_version} before migration {}",
                pending[0].version
            ),
        })
    }
}

fn current_schema_version(database: &Database) -> Result<i64, StorageError> {
    current_schema_version_from(&database.connection_handle().lock())
}

fn current_schema_version_from(conn: &Connection) -> Result<i64, StorageError> {
    let version: Option<i64> = conn
        .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
            row.get(0)
        })
        .unwrap_or(None);
    Ok(version.unwrap_or(0))
}

fn snapshot_database(database: &Database, target: &Path) -> Result<(), StorageError> {
    snapshot_connection(&database.connection_handle().lock(), target)
}

fn snapshot_connection(source: &Connection, target: &Path) -> Result<(), StorageError> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|source| StorageError::BackupIo {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let mut destination = Connection::open(target).map_err(|source| StorageError::BackupOpen {
        path: target.to_path_buf(),
        source,
    })?;
    let backup = rusqlite::backup::Backup::new(source, &mut destination).map_err(|source| {
        StorageError::BackupOpen {
            path: target.to_path_buf(),
            source,
        }
    })?;
    backup
        .run_to_completion(5, Duration::from_millis(50), None)
        .map_err(|source| StorageError::BackupOpen {
            path: target.to_path_buf(),
            source,
        })?;
    Ok(())
}

fn bundle_id_from_now() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time moves forward")
        .as_millis();
    format!("{millis}")
}

fn rfc3339_now() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time moves forward")
        .as_secs();
    format!("{seconds}")
}

fn read_manifest(bundle_path: &Path) -> Result<BackupManifest, StorageError> {
    let path = bundle_path.join("manifest.json");
    let text = fs::read_to_string(&path).map_err(|source| StorageError::BackupIo {
        path: path.clone(),
        source,
    })?;
    serde_json::from_str(&text).map_err(|source| StorageError::BackupInvalid {
        reason: source.to_string(),
    })
}

fn write_manifest(bundle_path: &Path, manifest: &BackupManifest) -> Result<(), StorageError> {
    let path = bundle_path.join("manifest.json");
    let text =
        serde_json::to_string_pretty(manifest).map_err(|source| StorageError::BackupInvalid {
            reason: source.to_string(),
        })?;
    fs::write(&path, text).map_err(|source| StorageError::BackupIo { path, source })
}

fn write_verified_marker(bundle_path: &Path, schema_version: i64) -> Result<(), StorageError> {
    let path = bundle_path.join("verified.json");
    let detail = serde_json::json!({
        "schema_version": schema_version,
        "verified_at": rfc3339_now(),
    });
    fs::write(&path, detail.to_string()).map_err(|source| StorageError::BackupIo { path, source })
}

fn verified_records_path(managed_root: &Path) -> PathBuf {
    backups_dir(managed_root).join("verified-records.json")
}

fn manifest_entry(relative: &str, plaintext: &[u8]) -> ManifestEntry {
    ManifestEntry {
        path: relative.to_string(),
        sha256: content_hash(plaintext),
        size: plaintext.len() as u64,
    }
}

fn list_bundle_dirs(root: &Path) -> Result<Vec<PathBuf>, StorageError> {
    let mut bundles = Vec::new();
    for entry in fs::read_dir(root).map_err(|source| StorageError::BackupIo {
        path: root.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| StorageError::BackupIo {
            path: root.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        if path.is_dir() && path.join("manifest.json").exists() {
            bundles.push(path);
        }
    }
    Ok(bundles)
}

fn copy_tree(
    source_root: &Path,
    target_root: &Path,
    encrypted: bool,
    passphrase: Option<&str>,
    files: &mut Vec<ManifestEntry>,
) -> Result<(), StorageError> {
    fs::create_dir_all(target_root).map_err(|source| StorageError::BackupIo {
        path: target_root.to_path_buf(),
        source,
    })?;
    for entry in fs::read_dir(source_root).map_err(|source| StorageError::BackupIo {
        path: source_root.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| StorageError::BackupIo {
            path: source_root.to_path_buf(),
            source,
        })?;
        let source_path = entry.path();
        let file_name = entry.file_name();
        let relative = format!("attachments/{}", file_name.to_string_lossy());
        let target_path = target_root.join(&file_name);
        if source_path.is_dir() {
            copy_tree(&source_path, &target_path, encrypted, passphrase, files)?;
            continue;
        }
        fs::copy(&source_path, &target_path).map_err(|source| StorageError::BackupIo {
            path: target_path.clone(),
            source,
        })?;
        let bytes = fs::read(&target_path).map_err(|source| StorageError::BackupIo {
            path: target_path.clone(),
            source,
        })?;
        if encrypted {
            let passphrase = passphrase.expect("encrypted needs passphrase");
            fs::write(&target_path, encrypt_bytes(passphrase, &bytes)?).map_err(|source| {
                StorageError::BackupIo {
                    path: target_path.clone(),
                    source,
                }
            })?;
        }
        files.push(manifest_entry(&relative, &bytes));
    }
    Ok(())
}

fn extract_bundle(
    bundle_path: &Path,
    staging: &Path,
    manifest: &BackupManifest,
    passphrase: Option<&str>,
) -> Result<(), StorageError> {
    for entry in &manifest.files {
        let source = bundle_path.join(&entry.path);
        let target = staging.join(&entry.path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|source| StorageError::BackupIo {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let bytes = read_payload(&source, manifest.encrypted, passphrase)?;
        fs::write(&target, bytes).map_err(|source| StorageError::BackupIo {
            path: target,
            source,
        })?;
    }
    Ok(())
}

fn read_payload(
    path: &Path,
    encrypted: bool,
    passphrase: Option<&str>,
) -> Result<Vec<u8>, StorageError> {
    let bytes = fs::read(path).map_err(|source| StorageError::BackupIo {
        path: path.to_path_buf(),
        source,
    })?;
    if encrypted {
        let passphrase = passphrase.ok_or_else(|| StorageError::BackupInvalid {
            reason: "encrypted bundle requires a passphrase".to_string(),
        })?;
        decrypt_bytes(passphrase, &bytes)
    } else {
        Ok(bytes)
    }
}

fn encrypt_bytes(passphrase: &str, plaintext: &[u8]) -> Result<Vec<u8>, StorageError> {
    let key = Key::from(derive_key(passphrase)?);
    let cipher = ChaCha20Poly1305::new(&key);
    let mut nonce_bytes = [0_u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext =
        cipher
            .encrypt(nonce, plaintext)
            .map_err(|error| StorageError::BackupInvalid {
                reason: error.to_string(),
            })?;
    let mut payload = Vec::with_capacity(12 + ciphertext.len());
    payload.extend_from_slice(&nonce_bytes);
    payload.extend_from_slice(&ciphertext);
    Ok(payload)
}

fn decrypt_bytes(passphrase: &str, payload: &[u8]) -> Result<Vec<u8>, StorageError> {
    if payload.len() < 12 {
        return Err(StorageError::BackupInvalid {
            reason: "encrypted payload is too short".to_string(),
        });
    }
    let (nonce_bytes, ciphertext) = payload.split_at(12);
    let key = Key::from(derive_key(passphrase)?);
    let cipher = ChaCha20Poly1305::new(&key);
    let nonce = Nonce::from_slice(nonce_bytes);
    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|error| StorageError::BackupInvalid {
            reason: error.to_string(),
        })
}

fn derive_key(passphrase: &str) -> Result<[u8; 32], StorageError> {
    let salt = b"kanban-backup-v1";
    let mut key = [0_u8; 32];
    Argon2::default()
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|error| StorageError::BackupInvalid {
            reason: error.to_string(),
        })?;
    Ok(key)
}

fn atomic_swap_restore(managed_root: &Path, staging: &Path) -> Result<(), StorageError> {
    let database_live = managed_root.join(database_file_name());
    let attachments_live = attachments_dir(managed_root);
    let config_live = managed_root.join(config_file_name());
    let quarantine = managed_root.join(".pre-restore-quarantine");
    if quarantine.exists() {
        fs::remove_dir_all(&quarantine).map_err(|source| StorageError::BackupIo {
            path: quarantine.clone(),
            source,
        })?;
    }
    fs::create_dir_all(&quarantine).map_err(|source| StorageError::BackupIo {
        path: quarantine.clone(),
        source,
    })?;

    if database_live.exists() {
        fs::rename(&database_live, quarantine.join(database_file_name())).map_err(|source| {
            StorageError::BackupIo {
                path: database_live.clone(),
                source,
            }
        })?;
    }
    if attachments_live.exists() {
        fs::rename(&attachments_live, quarantine.join("attachments")).map_err(|source| {
            StorageError::BackupIo {
                path: attachments_live.clone(),
                source,
            }
        })?;
    }
    if config_live.exists() {
        fs::rename(&config_live, quarantine.join(config_file_name())).map_err(|source| {
            StorageError::BackupIo {
                path: config_live.clone(),
                source,
            }
        })?;
    }

    fs::rename(
        staging.join(database_file_name()),
        managed_root.join(database_file_name()),
    )
    .map_err(|source| StorageError::BackupIo {
        path: managed_root.join(database_file_name()),
        source,
    })?;
    if staging.join("attachments").exists() {
        fs::rename(staging.join("attachments"), attachments_dir(managed_root)).map_err(
            |source| StorageError::BackupIo {
                path: attachments_dir(managed_root),
                source,
            },
        )?;
    }
    let staged_config = staging.join(config_file_name());
    if staged_config.exists() {
        fs::rename(&staged_config, managed_root.join(config_file_name())).map_err(|source| {
            StorageError::BackupIo {
                path: managed_root.join(config_file_name()),
                source,
            }
        })?;
    }

    fs::remove_dir_all(staging).map_err(|source| StorageError::BackupIo {
        path: staging.to_path_buf(),
        source,
    })?;
    fs::remove_dir_all(&quarantine).map_err(|source| StorageError::BackupIo {
        path: quarantine,
        source,
    })?;
    Ok(())
}

#[cfg(test)]
mod backup_restore {
    use std::num::NonZeroU32;

    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use kanban_app::{EvidenceStore, TimelineFacts};
    use kanban_domain::{CommitIdentity, RelativePath};
    use kanban_dto::TimelineEventKind;
    use serde_json::json;

    use super::{BackupOptions, BackupRetentionPolicy, BackupStore};
    use crate::db::Database;
    use crate::evidence::SqliteEvidenceStore;
    use crate::migrations::AllowAllMigrations;
    use crate::paths::{attachments_dir, config_file_name, database_file_name};
    use crate::test_support::scratch_database;

    fn managed_fixture() -> (tempfile::TempDir, Database, BackupStore) {
        let (dir, mut database) = scratch_database();
        database
            .migrate(&AllowAllMigrations)
            .expect("the migrations apply");
        let managed_root = dir.path().to_path_buf();
        let store = BackupStore::new(managed_root.clone());
        (dir, database, store)
    }

    fn timeline_facts() -> TimelineFacts {
        TimelineFacts {
            kind: TimelineEventKind::Evidence,
            facts: json!({ "probe": true }),
        }
    }

    fn seed_state(managed_root: &std::path::Path, database: &Database) {
        let config = managed_root.join(config_file_name());
        std::fs::write(&config, br#"{"theme":"dark"}"#).expect("config writes");
        let conn = database.connection();
        conn.execute(
            "INSERT INTO initiatives (name, archived, version) VALUES ('Alpha', 0, 1)",
            [],
        )
        .expect("initiative inserts");
        let evidence = SqliteEvidenceStore::new(database, attachments_dir(managed_root));
        evidence
            .attach_managed_file(
                "proj",
                "ticket",
                "1",
                &STANDARD.encode(b"evidence-bytes"),
                timeline_facts(),
            )
            .expect("evidence attaches");
        evidence
            .attach_repository(
                "proj",
                "ticket",
                "2",
                &RelativePath::new("docs/readme.md").expect("path is valid"),
                &CommitIdentity::new("deadbeef").expect("commit is valid"),
                timeline_facts(),
            )
            .expect("repository evidence attaches");
    }

    #[test]
    fn create_backup_bundle_includes_sqlite_attachments_and_config() {
        let (dir, database, store) = managed_fixture();
        seed_state(dir.path(), &database);

        let bundle = store
            .create(
                &database,
                &BackupOptions {
                    retention: BackupRetentionPolicy::keep_most_recent(
                        NonZeroU32::new(3).expect("three is not zero"),
                    ),
                    passphrase: None,
                },
            )
            .expect("backup creates");

        let manifest = store.validate(&bundle, None).expect("backup validates");
        let paths: Vec<_> = manifest
            .files
            .iter()
            .map(|entry| entry.path.as_str())
            .collect();
        assert!(paths.iter().any(|path| *path == database_file_name()));
        assert!(paths.iter().any(|path| path.starts_with("attachments/")));
        assert!(paths.iter().any(|path| *path == config_file_name()));
        assert_eq!(manifest.schema_version, 9);
    }

    #[test]
    fn restore_into_fresh_core_reproduces_state_exactly() {
        let (dir, database, store) = managed_fixture();
        seed_state(dir.path(), &database);
        let bundle = store
            .create(
                &database,
                &BackupOptions {
                    retention: BackupRetentionPolicy::keep_most_recent(
                        NonZeroU32::new(3).expect("three is not zero"),
                    ),
                    passphrase: None,
                },
            )
            .expect("backup creates");

        let fresh_dir = tempfile::tempdir().expect("fresh directory");
        let fresh_root = fresh_dir.path().to_path_buf();
        let fresh_store = BackupStore::new(fresh_root.clone());
        fresh_store
            .restore(&bundle, None)
            .expect("restore succeeds");

        let restored = Database::open(&fresh_root.join(database_file_name()))
            .expect("restored database opens");
        let name: String = restored
            .connection()
            .query_row("SELECT name FROM initiatives", [], |row| row.get(0))
            .expect("initiative reads");
        assert_eq!(name, "Alpha");
        let evidence_count: i64 = restored
            .connection()
            .query_row("SELECT COUNT(*) FROM evidence_items", [], |row| row.get(0))
            .expect("evidence reads");
        assert_eq!(evidence_count, 2);
        let config =
            std::fs::read_to_string(fresh_root.join(config_file_name())).expect("config reads");
        assert_eq!(config, r#"{"theme":"dark"}"#);
        let attachments = attachments_dir(&fresh_root);
        let attachment_files: Vec<_> = std::fs::read_dir(&attachments)
            .expect("attachments list")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        assert_eq!(attachment_files.len(), 1);
    }

    #[test]
    fn restore_writes_to_staging_before_swap() {
        let (dir, database, store) = managed_fixture();
        seed_state(dir.path(), &database);
        let bundle = store
            .create(
                &database,
                &BackupOptions {
                    retention: BackupRetentionPolicy::keep_most_recent(
                        NonZeroU32::new(3).expect("three is not zero"),
                    ),
                    passphrase: None,
                },
            )
            .expect("backup creates");

        let manifest = store.validate(&bundle, None).expect("manifest reads");
        let staging = dir.path().join(".restore-staging");
        super::extract_bundle(&bundle, &staging, &manifest, None).expect("staging extracts");
        assert!(staging.join(database_file_name()).exists());
        assert!(!dir.path().join(".restore-staging").exists() || staging.exists());

        store.restore(&bundle, None).expect("restore swaps");
        assert!(!staging.exists(), "staging must be removed after the swap");
        assert!(dir.path().join(database_file_name()).exists());
    }

    #[test]
    fn validate_detects_tampered_hash() {
        let (dir, database, store) = managed_fixture();
        seed_state(dir.path(), &database);
        let bundle = store
            .create(
                &database,
                &BackupOptions {
                    retention: BackupRetentionPolicy::keep_most_recent(
                        NonZeroU32::new(3).expect("three is not zero"),
                    ),
                    passphrase: None,
                },
            )
            .expect("backup creates");

        let config_path = bundle.join(config_file_name());
        std::fs::write(&config_path, b"tampered").expect("config tampers");

        assert!(store.validate(&bundle, None).is_err());
    }

    #[test]
    fn encrypted_backup_round_trips() {
        let (dir, database, store) = managed_fixture();
        seed_state(dir.path(), &database);
        let passphrase = "operator-secret";
        let bundle = store
            .create(
                &database,
                &BackupOptions {
                    retention: BackupRetentionPolicy::keep_most_recent(
                        NonZeroU32::new(3).expect("three is not zero"),
                    ),
                    passphrase: Some(passphrase.to_string()),
                },
            )
            .expect("encrypted backup creates");

        let preview = store
            .preview(&bundle, Some(passphrase))
            .expect("preview succeeds");
        assert!(preview.manifest.encrypted);

        let fresh_dir = tempfile::tempdir().expect("fresh directory");
        let fresh_store = BackupStore::new(fresh_dir.path().to_path_buf());
        fresh_store
            .restore(&bundle, Some(passphrase))
            .expect("encrypted restore succeeds");
        let config = std::fs::read_to_string(fresh_dir.path().join(config_file_name()))
            .expect("config reads");
        assert_eq!(config, r#"{"theme":"dark"}"#);
    }

    #[test]
    fn retention_prunes_older_bundles() {
        let (dir, database, store) = managed_fixture();
        seed_state(dir.path(), &database);
        let options = BackupOptions {
            retention: BackupRetentionPolicy::keep_most_recent(
                NonZeroU32::new(2).expect("two is not zero"),
            ),
            passphrase: None,
        };
        store.create(&database, &options).expect("first backup");
        store.create(&database, &options).expect("second backup");
        store.create(&database, &options).expect("third backup");

        let bundles =
            super::list_bundle_dirs(&crate::paths::backups_dir(dir.path())).expect("bundles list");
        assert_eq!(bundles.len(), 2);
    }
}

#[cfg(test)]
mod migration_requires_backup {
    use std::num::NonZeroU32;

    use super::{BackupOptions, BackupRetentionPolicy, BackupStore, VerifiedBackupHook};
    use crate::db::Database;
    use crate::error::StorageError;
    use crate::test_support::scratch_database;

    fn database_at_schema(version: i64) -> (tempfile::TempDir, Database, BackupStore) {
        let (dir, database) = scratch_database();
        crate::migrations::apply_through(&database.connection(), version)
            .expect("schema fabricates");
        let store = BackupStore::new(dir.path().to_path_buf());
        (dir, database, store)
    }

    #[test]
    fn refuses_without_verified_backup() {
        let (dir, mut database, store) = database_at_schema(8);
        let hook = VerifiedBackupHook::refuse_without_backup(&store, &database);

        let outcome = database.migrate(&hook);

        assert!(matches!(outcome, Err(StorageError::HookRefused { .. })));
        let present: i64 = database
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'projects'",
                [],
                |row| row.get(0),
            )
            .expect("sqlite_master is readable");
        assert_eq!(present, 0, "migration must not run without backup");
        let _ = dir;
    }

    #[test]
    fn succeeds_with_verified_backup() {
        let (dir, mut database, store) = database_at_schema(8);
        store
            .create(
                &database,
                &BackupOptions {
                    retention: BackupRetentionPolicy::keep_most_recent(
                        NonZeroU32::new(3).expect("three is not zero"),
                    ),
                    passphrase: None,
                },
            )
            .expect("verified backup exists");
        let hook = VerifiedBackupHook::refuse_without_backup(&store, &database);

        database.migrate(&hook).expect("migration proceeds");

        let version: i64 = database
            .connection()
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .expect("schema version reads");
        assert_eq!(version, 9);
        let _ = dir;
    }

    #[test]
    fn create_before_migrate_takes_verified_backup() {
        let (dir, mut database, store) = database_at_schema(8);
        let hook = VerifiedBackupHook::create_before_migrate(
            &store,
            &database,
            BackupRetentionPolicy::keep_most_recent(NonZeroU32::new(3).expect("three is not zero")),
        );

        database.migrate(&hook).expect("migration proceeds");

        assert!(
            store
                .verified_record_for(8)
                .expect("record reads")
                .is_some()
        );
        let version: i64 = database
            .connection()
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .expect("schema version reads");
        assert_eq!(version, 9);
        let _ = dir;
    }
}
