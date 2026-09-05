//! Consistent backup bundles of SQLite, attachments, and
//! configuration, with manifest hashes, validation, encryption,
//! preview, retention, and safe restore (KAN-S13-US2).

use std::fs;
use std::num::NonZeroU32;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

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
    /// Hex-encoded Argon2 salt; present when encrypted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encryption_salt: Option<String>,
    /// Every file in the bundle and its content hash.
    pub files: Vec<ManifestEntry>,
}

/// Operator backup settings read from managed configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackupSettings {
    /// How many verified bundles to keep.
    pub retention: BackupRetentionPolicy,
}

impl BackupSettings {
    /// The default retention when configuration is absent.
    pub const fn default_retention() -> BackupRetentionPolicy {
        BackupRetentionPolicy::keep_most_recent(NonZeroU32::new(7).expect("seven is not zero"))
    }

    /// The product default settings.
    pub const fn product_default() -> Self {
        Self {
            retention: Self::default_retention(),
        }
    }
}

impl Default for BackupSettings {
    fn default() -> Self {
        Self::product_default()
    }
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
        validate_manifest(&manifest)?;
        let salt = encryption_salt_bytes(&manifest)?;
        for entry in &manifest.files {
            let path = bundle_path.join(&entry.path);
            let bytes = read_payload(&path, manifest.encrypted, passphrase, salt.as_deref())?;
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
            let staging = SecureValidationTemp::new(bundle_path)?;
            let bytes = read_payload(&database_path, true, passphrase, salt.as_deref())?;
            fs::write(staging.path(), &bytes).map_err(|source| StorageError::BackupIo {
                path: staging.path().to_path_buf(),
                source,
            })?;
            validation_temp_test_hooks::after_staging_write(staging.path())?;
            let conn =
                Connection::open(staging.path()).map_err(|source| StorageError::BackupOpen {
                    path: staging.path().to_path_buf(),
                    source,
                })?;
            let check: String = conn
                .query_row("PRAGMA integrity_check", [], |row| row.get(0))
                .map_err(|source| StorageError::BackupOpen {
                    path: staging.path().to_path_buf(),
                    source,
                })?;
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
        let removed_ids: Vec<String> = removed
            .iter()
            .filter_map(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_owned)
            })
            .collect();
        self.prune_verified_records(&removed_ids)?;
        Ok(removed)
    }

    /// The verified backup for `schema_version`, if one exists.
    pub fn verified_record_for(
        &self,
        schema_version: i64,
    ) -> Result<Option<VerifiedBackupRecord>, StorageError> {
        let records = self.load_verified_records()?;
        let Some(record) = records
            .into_iter()
            .find(|record| record.schema_version == schema_version)
        else {
            return Ok(None);
        };
        let bundle_path = backups_dir(&self.managed_root).join(&record.bundle_id);
        if bundle_path.exists() {
            return Ok(Some(record));
        }
        self.prune_verified_records(&[record.bundle_id])?;
        Ok(None)
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
        let encryption_salt = if encrypted {
            Some(generate_encryption_salt())
        } else {
            None
        };
        let salt_bytes = encryption_salt
            .as_deref()
            .map(encryption_salt_from_hex)
            .transpose()?;
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
                encrypt_bytes(
                    passphrase,
                    salt_bytes.as_deref().expect("encrypted needs salt"),
                    &database_bytes,
                )?,
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
                salt_bytes.as_deref(),
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
                fs::write(
                    &bundle_config,
                    encrypt_bytes(
                        passphrase,
                        salt_bytes.as_deref().expect("encrypted needs salt"),
                        &config_bytes,
                    )?,
                )
                .map_err(|source| StorageError::BackupIo {
                    path: bundle_config.clone(),
                    source,
                })?;
            }
        }

        let manifest = BackupManifest {
            format_version: 1,
            created_at: rfc3339_now(),
            schema_version,
            encrypted,
            encryption_salt,
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
        validate_manifest(manifest)?;
        for entry in &manifest.files {
            let path = resolve_under(staging, &entry.path)?;
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
        if !database_path.is_file() {
            return Err(StorageError::BackupInvalid {
                reason: format!(
                    "restore staging is missing required file {}",
                    database_file_name()
                ),
            });
        }
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

    fn prune_verified_records(&self, removed_bundle_ids: &[String]) -> Result<(), StorageError> {
        if removed_bundle_ids.is_empty() {
            return Ok(());
        }
        let mut records = self.load_verified_records()?;
        let before = records.len();
        records.retain(|record| !removed_bundle_ids.contains(&record.bundle_id));
        if records.len() != before {
            self.save_verified_records(&records)?;
        }
        Ok(())
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
        let encryption_salt = if encrypted {
            Some(generate_encryption_salt())
        } else {
            None
        };
        let salt_bytes = encryption_salt
            .as_deref()
            .map(encryption_salt_from_hex)
            .transpose()?;
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
                encrypt_bytes(
                    passphrase,
                    salt_bytes.as_deref().expect("encrypted needs salt"),
                    &database_bytes,
                )?,
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
                salt_bytes.as_deref(),
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
                fs::write(
                    &bundle_config,
                    encrypt_bytes(
                        passphrase,
                        salt_bytes.as_deref().expect("encrypted needs salt"),
                        &config_bytes,
                    )?,
                )
                .map_err(|source| StorageError::BackupIo {
                    path: bundle_config.clone(),
                    source,
                })?;
            }
        }

        let manifest = BackupManifest {
            format_version: 1,
            created_at: rfc3339_now(),
            schema_version,
            encrypted,
            encryption_salt,
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

/// Reads operator backup settings from managed configuration.
pub fn load_backup_settings(managed_root: &Path) -> BackupSettings {
    let config_path = managed_root.join(config_file_name());
    let Ok(text) = fs::read_to_string(&config_path) else {
        return BackupSettings::default();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return BackupSettings::default();
    };
    let Some(retained) = value
        .get("backup_retention")
        .and_then(serde_json::Value::as_u64)
        .and_then(|count| NonZeroU32::new(count as u32))
    else {
        return BackupSettings::default();
    };
    BackupSettings {
        retention: BackupRetentionPolicy::keep_most_recent(retained),
    }
}

fn validate_manifest(manifest: &BackupManifest) -> Result<(), StorageError> {
    ensure_database_in_manifest(manifest)?;
    for entry in &manifest.files {
        validate_manifest_path(&entry.path)?;
    }
    if manifest.encrypted && manifest.encryption_salt.is_none() {
        return Err(StorageError::BackupInvalid {
            reason: "encrypted bundle is missing its authentication salt".to_string(),
        });
    }
    Ok(())
}

fn ensure_database_in_manifest(manifest: &BackupManifest) -> Result<(), StorageError> {
    let database_name = database_file_name();
    if manifest
        .files
        .iter()
        .any(|entry| entry.path == database_name)
    {
        Ok(())
    } else {
        Err(StorageError::BackupInvalid {
            reason: format!("bundle manifest is missing required file {database_name}"),
        })
    }
}

fn validate_manifest_path(path: &str) -> Result<(), StorageError> {
    if path.is_empty() {
        return Err(StorageError::BackupInvalid {
            reason: "manifest path is empty".to_string(),
        });
    }
    if path.starts_with('/') || path.contains('\\') {
        return Err(StorageError::BackupInvalid {
            reason: format!("manifest path is not bundle-relative: {path}"),
        });
    }
    for component in Path::new(path).components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            _ => {
                return Err(StorageError::BackupInvalid {
                    reason: format!("manifest path escapes the bundle root: {path}"),
                });
            }
        }
    }
    Ok(())
}

fn resolve_under(base: &Path, relative: &str) -> Result<PathBuf, StorageError> {
    validate_manifest_path(relative)?;
    let mut resolved = base.to_path_buf();
    for component in Path::new(relative).components() {
        match component {
            Component::Normal(part) => resolved.push(part),
            Component::CurDir => {}
            _ => {
                return Err(StorageError::BackupInvalid {
                    reason: format!("manifest path escapes the bundle root: {relative}"),
                });
            }
        }
    }
    Ok(resolved)
}

fn sqlite_sidecar_paths(database_path: &Path) -> [PathBuf; 2] {
    let path = database_path.to_string_lossy();
    [
        PathBuf::from(format!("{path}-wal")),
        PathBuf::from(format!("{path}-shm")),
    ]
}

fn remove_sqlite_sidecars(database_path: &Path) -> Result<(), StorageError> {
    for sidecar in sqlite_sidecar_paths(database_path) {
        if sidecar.exists() {
            fs::remove_file(&sidecar).map_err(|source| StorageError::BackupIo {
                path: sidecar,
                source,
            })?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod validation_temp_test_hooks {
    use std::cell::RefCell;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    thread_local! {
        static TEMP_ROOT: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
        static CORRUPT_AFTER_WRITE: RefCell<bool> = const { RefCell::new(false) };
        static OBSERVED_STAGING: RefCell<Vec<(PathBuf, u32)>> = const { RefCell::new(Vec::new()) };
    }

    pub struct ValidationTempRootGuard {
        active: bool,
    }

    impl ValidationTempRootGuard {
        pub fn set(root: PathBuf) -> Self {
            set_validation_temp_root(root);
            Self { active: true }
        }
    }

    impl Drop for ValidationTempRootGuard {
        fn drop(&mut self) {
            if self.active {
                TEMP_ROOT.with(|slot| *slot.borrow_mut() = None);
                CORRUPT_AFTER_WRITE.with(|flag| *flag.borrow_mut() = false);
            }
        }
    }

    pub struct CorruptStagingAfterWriteGuard {
        active: bool,
    }

    impl CorruptStagingAfterWriteGuard {
        pub fn enable() -> Self {
            CORRUPT_AFTER_WRITE.with(|flag| *flag.borrow_mut() = true);
            Self { active: true }
        }
    }

    impl Drop for CorruptStagingAfterWriteGuard {
        fn drop(&mut self) {
            if self.active {
                CORRUPT_AFTER_WRITE.with(|flag| *flag.borrow_mut() = false);
            }
        }
    }

    pub fn validation_temp_root() -> Option<PathBuf> {
        TEMP_ROOT.with(|slot| slot.borrow().clone())
    }

    pub fn set_validation_temp_root(root: PathBuf) {
        TEMP_ROOT.with(|slot| *slot.borrow_mut() = Some(root));
    }

    pub fn corrupt_after_write() -> bool {
        CORRUPT_AFTER_WRITE.with(|flag| *flag.borrow())
    }

    pub fn take_observed_staging() -> Vec<(PathBuf, u32)> {
        OBSERVED_STAGING.with(|observed| std::mem::take(&mut *observed.borrow_mut()))
    }

    pub fn after_staging_write(path: &Path) -> Result<(), super::StorageError> {
        #[cfg(unix)]
        {
            let mode = std::fs::metadata(path)
                .map_err(|source| super::StorageError::BackupIo {
                    path: path.to_path_buf(),
                    source,
                })?
                .permissions()
                .mode()
                & 0o777;
            OBSERVED_STAGING.with(|observed| {
                observed.borrow_mut().push((path.to_path_buf(), mode));
            });
        }
        if corrupt_after_write() {
            let conn = super::Connection::open(path).map_err(|source| {
                super::StorageError::BackupOpen {
                    path: path.to_path_buf(),
                    source,
                }
            })?;
            drop(conn);
            for sidecar in super::sqlite_sidecar_paths(path) {
                std::fs::write(&sidecar, b"stale-sidecar").map_err(|source| {
                    super::StorageError::BackupIo {
                        path: sidecar,
                        source,
                    }
                })?;
            }
            let mut bytes =
                std::fs::read(path).map_err(|source| super::StorageError::BackupIo {
                    path: path.to_path_buf(),
                    source,
                })?;
            if bytes.len() > 32 {
                bytes[16] ^= 0xFF;
            }
            std::fs::write(path, &bytes).map_err(|source| super::StorageError::BackupIo {
                path: path.to_path_buf(),
                source,
            })?;
        }
        Ok(())
    }
}

#[cfg(not(test))]
mod validation_temp_test_hooks {
    use std::path::{Path, PathBuf};

    pub fn validation_temp_root() -> Option<PathBuf> {
        None
    }

    pub fn after_staging_write(_path: &Path) -> Result<(), super::StorageError> {
        Ok(())
    }
}

struct SecureValidationTemp {
    file: tempfile::NamedTempFile,
}

impl SecureValidationTemp {
    fn new(bundle_path: &Path) -> Result<Self, StorageError> {
        let temp_root = match validation_temp_test_hooks::validation_temp_root() {
            Some(root) => root,
            None => std::env::temp_dir(),
        };
        fs::create_dir_all(&temp_root).map_err(|source| StorageError::BackupIo {
            path: temp_root.clone(),
            source,
        })?;
        let file = tempfile::Builder::new()
            .prefix("kanban-validate-")
            .suffix(".sqlite")
            .tempfile_in(&temp_root)
            .map_err(|source| StorageError::BackupIo {
                path: bundle_path.to_path_buf(),
                source,
            })?;
        #[cfg(unix)]
        {
            fs::set_permissions(file.path(), fs::Permissions::from_mode(0o600)).map_err(
                |source| StorageError::BackupIo {
                    path: file.path().to_path_buf(),
                    source,
                },
            )?;
        }
        Ok(Self { file })
    }

    fn path(&self) -> &Path {
        self.file.path()
    }
}

impl Drop for SecureValidationTemp {
    fn drop(&mut self) {
        let path = self.file.path().to_path_buf();
        let _ = remove_sqlite_sidecars(&path);
    }
}

fn generate_encryption_salt() -> String {
    let mut salt = [0_u8; 16];
    OsRng.fill_bytes(&mut salt);
    encode_hex(&salt)
}

fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn is_hex_digit(ch: char) -> bool {
    ch.is_ascii_digit() || matches!(ch, 'a'..='f' | 'A'..='F')
}

fn encryption_salt_from_hex(encoded: &str) -> Result<Vec<u8>, StorageError> {
    if encoded.len() != 32 || !encoded.chars().all(is_hex_digit) {
        return Err(StorageError::BackupInvalid {
            reason: "encryption salt must be 16 bytes encoded as hex".to_string(),
        });
    }
    let mut salt = Vec::with_capacity(16);
    let bytes = encoded.as_bytes();
    for index in (0..bytes.len()).step_by(2) {
        let pair = std::str::from_utf8(&bytes[index..index + 2]).map_err(|error| {
            StorageError::BackupInvalid {
                reason: error.to_string(),
            }
        })?;
        salt.push(
            u8::from_str_radix(pair, 16).map_err(|error| StorageError::BackupInvalid {
                reason: error.to_string(),
            })?,
        );
    }
    Ok(salt)
}

fn encryption_salt_bytes(manifest: &BackupManifest) -> Result<Option<Vec<u8>>, StorageError> {
    if !manifest.encrypted {
        return Ok(None);
    }
    let encoded =
        manifest
            .encryption_salt
            .as_deref()
            .ok_or_else(|| StorageError::BackupInvalid {
                reason: "encrypted bundle is missing its authentication salt".to_string(),
            })?;
    Ok(Some(encryption_salt_from_hex(encoded)?))
}

#[cfg(test)]
pub(crate) mod restore_swap_test_hooks {
    use std::cell::Cell;

    pub const PHASE_QUARANTINE_READY: usize = 1;
    pub const PHASE_AFTER_DB_QUARANTINE: usize = 2;
    pub const PHASE_AFTER_ATTACHMENTS_QUARANTINE: usize = 3;
    pub const PHASE_AFTER_CONFIG_QUARANTINE: usize = 4;
    pub const PHASE_AFTER_DB_SWAP: usize = 5;
    pub const PHASE_AFTER_ATTACHMENTS_SWAP: usize = 6;
    pub const PHASE_AFTER_CONFIG_SWAP: usize = 7;

    thread_local! {
        static FAIL_AFTER_PHASE: Cell<usize> = const { Cell::new(0) };
    }

    pub fn set_fail_after_phase(phase: usize) {
        FAIL_AFTER_PHASE.with(|flag| flag.set(phase));
    }

    pub fn clear_fail_after_phase() {
        FAIL_AFTER_PHASE.with(|flag| flag.set(0));
    }

    pub struct FailAfterPhaseGuard {
        active: bool,
    }

    impl FailAfterPhaseGuard {
        pub fn set(phase: usize) -> Self {
            set_fail_after_phase(phase);
            Self { active: true }
        }
    }

    impl Drop for FailAfterPhaseGuard {
        fn drop(&mut self) {
            if self.active {
                clear_fail_after_phase();
            }
        }
    }

    pub fn maybe_fail(phase: usize) -> Result<(), super::StorageError> {
        let should_fail = FAIL_AFTER_PHASE.with(|flag| flag.get() == phase);
        if should_fail {
            return Err(super::StorageError::BackupInvalid {
                reason: format!("injected restore swap failure at phase {phase}"),
            });
        }
        Ok(())
    }
}

#[cfg(not(test))]
mod restore_swap_test_hooks {
    pub fn maybe_fail(_phase: usize) -> Result<(), super::StorageError> {
        Ok(())
    }
}

use restore_swap_test_hooks::maybe_fail;

struct RestoreSwapState {
    quarantined_database: bool,
    quarantined_attachments: bool,
    quarantined_config: bool,
    swapped_database: bool,
    swapped_attachments: bool,
    swapped_config: bool,
}

fn atomic_swap_restore(managed_root: &Path, staging: &Path) -> Result<(), StorageError> {
    let database_live = managed_root.join(database_file_name());
    let attachments_live = attachments_dir(managed_root);
    let config_live = managed_root.join(config_file_name());
    let quarantine = managed_root.join(".pre-restore-quarantine");
    let mut state = RestoreSwapState {
        quarantined_database: false,
        quarantined_attachments: false,
        quarantined_config: false,
        swapped_database: false,
        swapped_attachments: false,
        swapped_config: false,
    };

    let result = (|| {
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
        maybe_fail(1)?;

        remove_sqlite_sidecars(&database_live)?;

        if database_live.exists() {
            fs::rename(&database_live, quarantine.join(database_file_name())).map_err(
                |source| StorageError::BackupIo {
                    path: database_live.clone(),
                    source,
                },
            )?;
            state.quarantined_database = true;
            remove_sqlite_sidecars(&database_live)?;
        }
        maybe_fail(2)?;

        if attachments_live.exists() {
            fs::rename(&attachments_live, quarantine.join("attachments")).map_err(|source| {
                StorageError::BackupIo {
                    path: attachments_live.clone(),
                    source,
                }
            })?;
            state.quarantined_attachments = true;
        }
        maybe_fail(3)?;

        if config_live.exists() {
            fs::rename(&config_live, quarantine.join(config_file_name())).map_err(|source| {
                StorageError::BackupIo {
                    path: config_live.clone(),
                    source,
                }
            })?;
            state.quarantined_config = true;
        }
        maybe_fail(4)?;

        fs::rename(
            staging.join(database_file_name()),
            managed_root.join(database_file_name()),
        )
        .map_err(|source| StorageError::BackupIo {
            path: managed_root.join(database_file_name()),
            source,
        })?;
        state.swapped_database = true;
        remove_sqlite_sidecars(&database_live)?;
        maybe_fail(5)?;

        if staging.join("attachments").exists() {
            fs::rename(staging.join("attachments"), attachments_dir(managed_root)).map_err(
                |source| StorageError::BackupIo {
                    path: attachments_dir(managed_root),
                    source,
                },
            )?;
            state.swapped_attachments = true;
        }
        maybe_fail(6)?;

        let staged_config = staging.join(config_file_name());
        if staged_config.exists() {
            fs::rename(&staged_config, managed_root.join(config_file_name())).map_err(
                |source| StorageError::BackupIo {
                    path: managed_root.join(config_file_name()),
                    source,
                },
            )?;
            state.swapped_config = true;
        }
        maybe_fail(7)?;

        fs::remove_dir_all(staging).map_err(|source| StorageError::BackupIo {
            path: staging.to_path_buf(),
            source,
        })?;
        fs::remove_dir_all(&quarantine).map_err(|source| StorageError::BackupIo {
            path: quarantine.clone(),
            source,
        })?;
        Ok(())
    })();

    if result.is_err() {
        rollback_restore_swap(managed_root, staging, &quarantine, &state)?;
    }
    result
}

fn rollback_restore_swap(
    managed_root: &Path,
    staging: &Path,
    quarantine: &Path,
    state: &RestoreSwapState,
) -> Result<(), StorageError> {
    let database_live = managed_root.join(database_file_name());
    let attachments_live = attachments_dir(managed_root);
    let config_live = managed_root.join(config_file_name());

    if state.swapped_config && config_live.exists() {
        let _ = fs::rename(&config_live, staging.join(config_file_name()));
    }
    if state.swapped_attachments && attachments_live.exists() {
        let _ = fs::rename(&attachments_live, staging.join("attachments"));
    }
    if state.swapped_database && database_live.exists() {
        remove_sqlite_sidecars(&database_live)?;
        let _ = fs::rename(&database_live, staging.join(database_file_name()));
    }

    if state.quarantined_config && !config_live.exists() {
        let quarantined = quarantine.join(config_file_name());
        if quarantined.exists() {
            fs::rename(&quarantined, &config_live).map_err(|source| StorageError::BackupIo {
                path: config_live.clone(),
                source,
            })?;
        }
    }
    if state.quarantined_attachments && !attachments_live.exists() {
        let quarantined = quarantine.join("attachments");
        if quarantined.exists() {
            fs::rename(&quarantined, &attachments_live).map_err(|source| {
                StorageError::BackupIo {
                    path: attachments_live.clone(),
                    source,
                }
            })?;
        }
    }
    if state.quarantined_database && !database_live.exists() {
        let quarantined = quarantine.join(database_file_name());
        if quarantined.exists() {
            fs::rename(&quarantined, &database_live).map_err(|source| StorageError::BackupIo {
                path: database_live.clone(),
                source,
            })?;
            remove_sqlite_sidecars(&database_live)?;
        }
    }

    if quarantine.exists() {
        let _ = fs::remove_dir_all(quarantine);
    }
    Ok(())
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
    salt: Option<&[u8]>,
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
            copy_tree(
                &source_path,
                &target_path,
                encrypted,
                passphrase,
                salt,
                files,
            )?;
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
            fs::write(
                &target_path,
                encrypt_bytes(passphrase, salt.expect("encrypted needs salt"), &bytes)?,
            )
            .map_err(|source| StorageError::BackupIo {
                path: target_path.clone(),
                source,
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
    let salt = encryption_salt_bytes(manifest)?;
    for entry in &manifest.files {
        let source = bundle_path.join(&entry.path);
        let target = resolve_under(staging, &entry.path)?;
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|source| StorageError::BackupIo {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let bytes = read_payload(&source, manifest.encrypted, passphrase, salt.as_deref())?;
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
    salt: Option<&[u8]>,
) -> Result<Vec<u8>, StorageError> {
    let bytes = fs::read(path).map_err(|source| StorageError::BackupIo {
        path: path.to_path_buf(),
        source,
    })?;
    if encrypted {
        let passphrase = passphrase.ok_or_else(|| StorageError::BackupInvalid {
            reason: "encrypted bundle requires a passphrase".to_string(),
        })?;
        let salt = salt.ok_or_else(|| StorageError::BackupInvalid {
            reason: "encrypted bundle is missing its authentication salt".to_string(),
        })?;
        decrypt_bytes(passphrase, salt, &bytes)
    } else {
        Ok(bytes)
    }
}

fn encrypt_bytes(passphrase: &str, salt: &[u8], plaintext: &[u8]) -> Result<Vec<u8>, StorageError> {
    let key = Key::from(derive_key(passphrase, salt)?);
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

fn decrypt_bytes(passphrase: &str, salt: &[u8], payload: &[u8]) -> Result<Vec<u8>, StorageError> {
    if payload.len() < 12 {
        return Err(StorageError::BackupInvalid {
            reason: "encrypted payload is too short".to_string(),
        });
    }
    let (nonce_bytes, ciphertext) = payload.split_at(12);
    let key = Key::from(derive_key(passphrase, salt)?);
    let cipher = ChaCha20Poly1305::new(&key);
    let nonce = Nonce::from_slice(nonce_bytes);
    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|error| StorageError::BackupInvalid {
            reason: error.to_string(),
        })
}

fn derive_key(passphrase: &str, salt: &[u8]) -> Result<[u8; 32], StorageError> {
    let mut key = [0_u8; 32];
    Argon2::default()
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|error| StorageError::BackupInvalid {
            reason: error.to_string(),
        })?;
    Ok(key)
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
    use crate::migrations::{AllowAllMigrations, LATEST_SCHEMA_VERSION};
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
        assert_eq!(manifest.schema_version, LATEST_SCHEMA_VERSION);
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

    #[test]
    fn restore_removes_stale_wal_and_shm_and_recovers_database() {
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

        database
            .connection()
            .execute("UPDATE initiatives SET name = 'Beta' WHERE id = 1", [])
            .expect("live data mutates");
        let database_path = dir.path().join(database_file_name());
        let wal_path = super::sqlite_sidecar_paths(&database_path)[0].clone();
        let shm_path = super::sqlite_sidecar_paths(&database_path)[1].clone();
        assert!(
            wal_path.exists(),
            "WAL mode leaves a sidecar after mutation"
        );
        std::fs::write(&wal_path, b"stale-wal-frames").expect("wal tampers");
        std::fs::write(&shm_path, b"stale-shm").expect("shm tampers");

        store.restore(&bundle, None).expect("restore succeeds");

        for sidecar in super::sqlite_sidecar_paths(&database_path) {
            assert!(
                !sidecar.exists(),
                "restore must remove stale sidecars: {}",
                sidecar.display()
            );
        }
        let restored = Database::open(&database_path).expect("restored database opens");
        let name: String = restored
            .connection()
            .query_row("SELECT name FROM initiatives", [], |row| row.get(0))
            .expect("initiative reads");
        assert_eq!(name, "Alpha");
    }

    #[test]
    fn validate_rejects_bundle_missing_database_payload() {
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

        std::fs::remove_file(bundle.join(database_file_name())).expect("database payload removed");
        let manifest_path = bundle.join("manifest.json");
        let manifest_text = std::fs::read_to_string(&manifest_path).expect("manifest reads");
        let mut manifest: super::BackupManifest =
            serde_json::from_str(&manifest_text).expect("manifest decodes");
        manifest
            .files
            .retain(|entry| entry.path != database_file_name());
        std::fs::write(
            &manifest_path,
            serde_json::to_string_pretty(&manifest).expect("manifest encodes"),
        )
        .expect("manifest writes");

        let outcome = store.validate(&bundle, None);
        assert!(matches!(
            outcome,
            Err(crate::error::StorageError::BackupInvalid { .. })
        ));
    }

    #[test]
    fn restore_rejects_bundle_missing_database_manifest_entry() {
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

        std::fs::remove_file(bundle.join(database_file_name())).expect("database payload removed");
        let manifest_path = bundle.join("manifest.json");
        let manifest_text = std::fs::read_to_string(&manifest_path).expect("manifest reads");
        let mut manifest: super::BackupManifest =
            serde_json::from_str(&manifest_text).expect("manifest decodes");
        manifest
            .files
            .retain(|entry| entry.path != database_file_name());
        std::fs::write(
            &manifest_path,
            serde_json::to_string_pretty(&manifest).expect("manifest encodes"),
        )
        .expect("manifest writes");

        let outcome = store.restore(&bundle, None);
        assert!(matches!(
            outcome,
            Err(crate::error::StorageError::BackupInvalid { .. })
        ));
    }

    #[test]
    fn prune_removes_verified_records_for_removed_bundles() {
        let (dir, database, store) = managed_fixture();
        seed_state(dir.path(), &database);
        let options = BackupOptions {
            retention: BackupRetentionPolicy::keep_most_recent(
                NonZeroU32::new(1).expect("one is not zero"),
            ),
            passphrase: None,
        };
        store.create(&database, &options).expect("first backup");
        store.create(&database, &options).expect("second backup");

        assert!(
            store
                .verified_record_for(LATEST_SCHEMA_VERSION)
                .expect("records read")
                .is_some(),
            "the surviving bundle keeps a verified record"
        );
        let records_path = crate::paths::backups_dir(dir.path()).join("verified-records.json");
        let records_text = std::fs::read_to_string(&records_path).expect("records read");
        let records: Vec<super::VerifiedBackupRecord> =
            serde_json::from_str(&records_text).expect("records decode");
        assert_eq!(records.len(), 1);
        let bundles =
            super::list_bundle_dirs(&crate::paths::backups_dir(dir.path())).expect("bundles list");
        assert_eq!(bundles.len(), 1);
    }

    #[test]
    fn verified_record_for_ignores_pruned_bundle() {
        let (dir, database, store) = managed_fixture();
        seed_state(dir.path(), &database);
        let options = BackupOptions {
            retention: BackupRetentionPolicy::keep_most_recent(
                NonZeroU32::new(1).expect("one is not zero"),
            ),
            passphrase: None,
        };
        store.create(&database, &options).expect("first backup");
        store.create(&database, &options).expect("second backup");

        let record = store
            .verified_record_for(LATEST_SCHEMA_VERSION)
            .expect("record reads")
            .expect("verified record exists");
        let bundle_path = crate::paths::backups_dir(dir.path()).join(&record.bundle_id);
        assert!(bundle_path.exists());

        std::fs::remove_dir_all(&bundle_path).expect("bundle removed manually");
        assert!(
            store
                .verified_record_for(LATEST_SCHEMA_VERSION)
                .expect("record reads")
                .is_none(),
            "a missing bundle must not satisfy migration"
        );
    }

    #[test]
    fn validate_rejects_absolute_manifest_path() {
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

        let manifest_path = bundle.join("manifest.json");
        let manifest_text = std::fs::read_to_string(&manifest_path).expect("manifest reads");
        let mut manifest: super::BackupManifest =
            serde_json::from_str(&manifest_text).expect("manifest decodes");
        manifest.files.push(super::ManifestEntry {
            path: "/etc/passwd".to_string(),
            sha256: "00".repeat(32),
            size: 1,
        });
        std::fs::write(
            &manifest_path,
            serde_json::to_string_pretty(&manifest).expect("manifest encodes"),
        )
        .expect("manifest writes");

        let outcome = store.validate(&bundle, None);
        assert!(matches!(
            outcome,
            Err(crate::error::StorageError::BackupInvalid { .. })
        ));
    }

    #[test]
    fn validate_rejects_traversal_manifest_path() {
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

        let manifest_path = bundle.join("manifest.json");
        let manifest_text = std::fs::read_to_string(&manifest_path).expect("manifest reads");
        let mut manifest: super::BackupManifest =
            serde_json::from_str(&manifest_text).expect("manifest decodes");
        manifest.files.push(super::ManifestEntry {
            path: "../escape.txt".to_string(),
            sha256: "00".repeat(32),
            size: 1,
        });
        std::fs::write(
            &manifest_path,
            serde_json::to_string_pretty(&manifest).expect("manifest encodes"),
        )
        .expect("manifest writes");
        std::fs::write(bundle.join("../escape.txt"), b"escaped").expect("escape writes");

        let outcome = store.validate(&bundle, None);
        assert!(matches!(
            outcome,
            Err(crate::error::StorageError::BackupInvalid { .. })
        ));
    }

    #[test]
    fn restore_swap_rolls_back_on_injected_failures() {
        use super::restore_swap_test_hooks::{
            FailAfterPhaseGuard, PHASE_AFTER_ATTACHMENTS_QUARANTINE, PHASE_AFTER_ATTACHMENTS_SWAP,
            PHASE_AFTER_CONFIG_QUARANTINE, PHASE_AFTER_CONFIG_SWAP, PHASE_AFTER_DB_QUARANTINE,
            PHASE_AFTER_DB_SWAP, PHASE_QUARANTINE_READY,
        };

        let phases = [
            PHASE_QUARANTINE_READY,
            PHASE_AFTER_DB_QUARANTINE,
            PHASE_AFTER_ATTACHMENTS_QUARANTINE,
            PHASE_AFTER_CONFIG_QUARANTINE,
            PHASE_AFTER_DB_SWAP,
            PHASE_AFTER_ATTACHMENTS_SWAP,
            PHASE_AFTER_CONFIG_SWAP,
        ];

        for phase in phases {
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
            let managed_root = dir.path();
            let database_path = managed_root.join(database_file_name());
            let config_path = managed_root.join(config_file_name());
            let attachments_path = attachments_dir(managed_root);

            let _guard = FailAfterPhaseGuard::set(phase);
            let outcome = store.restore(&bundle, None);

            assert!(
                outcome.is_err(),
                "phase {phase} must inject a restore failure"
            );
            assert!(
                database_path.is_file(),
                "phase {phase} must roll the database back"
            );
            let name: String = database
                .connection()
                .query_row("SELECT name FROM initiatives", [], |row| row.get(0))
                .expect("live initiative reads");
            assert_eq!(name, "Alpha", "phase {phase} must preserve live data");
            assert!(config_path.is_file(), "phase {phase} must keep config");
            assert!(
                attachments_path.is_dir(),
                "phase {phase} must keep attachments"
            );
            assert!(
                !managed_root.join(".pre-restore-quarantine").exists(),
                "phase {phase} must clean quarantine"
            );
        }
    }

    #[test]
    fn encrypted_validation_leaves_no_plaintext_or_sidecars_in_bundle() {
        use std::os::unix::fs::PermissionsExt;

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

        store
            .validate(&bundle, Some(passphrase))
            .expect("encrypted validation succeeds");

        let forbidden = [
            ".validate-staging.sqlite",
            ".validate-staging.sqlite-wal",
            ".validate-staging.sqlite-shm",
        ];
        for name in forbidden {
            let path = bundle.join(name);
            assert!(
                !path.exists(),
                "validation must not leave artifacts in the bundle: {}",
                path.display()
            );
        }
        for entry in std::fs::read_dir(&bundle).expect("bundle lists") {
            let path = entry.expect("entry").path();
            if path.extension().and_then(|ext| ext.to_str()) == Some("sqlite")
                && path.file_name() != Some(std::ffi::OsStr::new(database_file_name()))
            {
                panic!(
                    "unexpected plaintext sqlite artifact in bundle: {}",
                    path.display()
                );
            }
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains("validate-staging"))
            {
                panic!("validation artifact leaked into bundle: {}", path.display());
            }
            if path.is_file() {
                let mode = std::fs::metadata(&path)
                    .expect("metadata reads")
                    .permissions()
                    .mode()
                    & 0o777;
                assert_ne!(
                    mode & 0o077,
                    0o077,
                    "bundle files must not be world-accessible: {}",
                    path.display()
                );
            }
        }
    }

    #[test]
    fn encrypted_bundles_use_distinct_authenticated_salts() {
        let (dir, database, store) = managed_fixture();
        seed_state(dir.path(), &database);
        let passphrase = "operator-secret";
        let first = store
            .create(
                &database,
                &BackupOptions {
                    retention: BackupRetentionPolicy::keep_most_recent(
                        NonZeroU32::new(3).expect("three is not zero"),
                    ),
                    passphrase: Some(passphrase.to_string()),
                },
            )
            .expect("first encrypted backup");
        let second = store
            .create(
                &database,
                &BackupOptions {
                    retention: BackupRetentionPolicy::keep_most_recent(
                        NonZeroU32::new(3).expect("three is not zero"),
                    ),
                    passphrase: Some(passphrase.to_string()),
                },
            )
            .expect("second encrypted backup");

        let first_manifest = store
            .validate(&first, Some(passphrase))
            .expect("first validates");
        let second_manifest = store
            .validate(&second, Some(passphrase))
            .expect("second validates");
        let first_salt = first_manifest
            .encryption_salt
            .expect("first bundle carries salt");
        let second_salt = second_manifest
            .encryption_salt
            .expect("second bundle carries salt");
        assert_ne!(first_salt, second_salt);
        assert_eq!(first_salt.len(), 32);
    }

    fn validation_artifacts_in(root: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut artifacts = Vec::new();
        if !root.exists() {
            return artifacts;
        }
        for entry in std::fs::read_dir(root).expect("validation root lists") {
            let path = entry.expect("entry").path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if name.starts_with("kanban-validate-")
                || name.ends_with(".sqlite-wal")
                || name.ends_with(".sqlite-shm")
            {
                artifacts.push(path);
            }
        }
        artifacts.sort();
        artifacts
    }

    #[test]
    fn encrypted_validation_temp_is_owner_only_and_cleans_up_after_integrity_failure() {
        use super::validation_temp_test_hooks::{
            CorruptStagingAfterWriteGuard, ValidationTempRootGuard, take_observed_staging,
        };

        let (dir, database, store) = managed_fixture();
        seed_state(dir.path(), &database);
        let validation_root = dir.path().join("validation-temp");
        std::fs::create_dir_all(&validation_root).expect("validation root creates");
        let _root_guard = ValidationTempRootGuard::set(validation_root.clone());
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
        let _corrupt_guard = CorruptStagingAfterWriteGuard::enable();
        let _ = take_observed_staging();

        let outcome = store.validate(&bundle, Some(passphrase));
        assert!(
            matches!(
                outcome,
                Err(crate::error::StorageError::BackupIntegrity { .. })
                    | Err(crate::error::StorageError::BackupOpen { .. })
            ),
            "validation must fail after staging decrypts: {outcome:?}"
        );

        let observed = take_observed_staging();
        assert_eq!(observed.len(), 1, "staging must be created before failure");
        let (staging_path, mode) = &observed[0];
        assert!(
            staging_path.starts_with(&validation_root),
            "staging must live in the isolated validation root"
        );
        assert_eq!(*mode, 0o600, "staging must be owner-only");
        assert!(
            validation_artifacts_in(&validation_root).is_empty(),
            "validation must remove plaintext and sqlite sidecars on failure"
        );
    }

    #[test]
    fn encrypted_validation_temp_roots_do_not_collide_across_parallel_runs() {
        use std::path::PathBuf;
        use std::sync::{Arc, Barrier, Mutex};
        use std::thread;

        use super::validation_temp_test_hooks::{
            CorruptStagingAfterWriteGuard, ValidationTempRootGuard,
        };

        let roots: Arc<Mutex<Vec<PathBuf>>> = Arc::new(Mutex::new(Vec::new()));
        let barrier = Arc::new(Barrier::new(2));
        let handles: Vec<_> = (0..2)
            .map(|index| {
                let roots = Arc::clone(&roots);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    let isolation = tempfile::tempdir().expect("isolation directory");
                    let validation_root = isolation.path().join(format!("branch-{index}"));
                    std::fs::create_dir_all(&validation_root).expect("validation root creates");
                    roots
                        .lock()
                        .expect("validation roots lock")
                        .push(validation_root.clone());
                    let _root_guard = ValidationTempRootGuard::set(validation_root.clone());
                    let (dir, database, store) = managed_fixture();
                    seed_state(dir.path(), &database);
                    let passphrase = format!("operator-secret-{index}");
                    let bundle = store
                        .create(
                            &database,
                            &BackupOptions {
                                retention: BackupRetentionPolicy::keep_most_recent(
                                    NonZeroU32::new(3).expect("three is not zero"),
                                ),
                                passphrase: Some(passphrase.clone()),
                            },
                        )
                        .expect("encrypted backup creates");
                    let _corrupt_guard = CorruptStagingAfterWriteGuard::enable();
                    barrier.wait();
                    assert!(
                        store.validate(&bundle, Some(&passphrase)).is_err(),
                        "branch clone {index} must fail after staging"
                    );
                    assert!(
                        validation_artifacts_in(&validation_root).is_empty(),
                        "branch clone {index} must clean its own validation root"
                    );
                })
            })
            .collect();
        for handle in handles {
            handle.join().expect("parallel validation joins");
        }
        let registered = roots.lock().expect("validation roots lock");
        assert_eq!(registered.len(), 2);
        assert_ne!(registered[0], registered[1]);
        for root in registered.iter() {
            assert!(
                validation_artifacts_in(root).is_empty(),
                "parallel runs must not leave validation artifacts behind"
            );
        }
    }
}

#[cfg(test)]
mod migration_requires_backup {
    use std::num::NonZeroU32;

    use super::{BackupOptions, BackupRetentionPolicy, BackupStore, VerifiedBackupHook};
    use crate::db::Database;
    use crate::error::StorageError;
    use crate::migrations::LATEST_SCHEMA_VERSION;
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
        assert_eq!(version, LATEST_SCHEMA_VERSION);
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
        assert_eq!(version, LATEST_SCHEMA_VERSION);
        let _ = dir;
    }

    #[test]
    fn refuses_when_verified_record_points_at_pruned_bundle() {
        let (dir, mut database, store) = database_at_schema(8);
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
            .expect("verified backup exists");
        std::fs::remove_dir_all(&bundle).expect("bundle pruned without record cleanup");
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
        assert_eq!(
            present, 0,
            "a stale verified record must not unlock migration"
        );
        let _ = dir;
    }
}
