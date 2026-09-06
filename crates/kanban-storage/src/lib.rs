//! SQLite implementation of the storage ports: connections,
//! forward-only migrations, and the append-only audit tables.

pub mod audit;
pub mod backup;
pub mod capacity;
pub mod clone_guard;
pub mod comments;
pub mod db;
pub mod deferrals;
pub mod dependencies;
pub mod dispatch;
pub mod error;
pub mod evidence;
pub mod graph_proposals;
pub mod herdr;
pub mod idempotency;
pub mod initiatives;
pub mod lanes;
pub mod migrations;
pub mod paths;
pub mod plan;
pub mod profiles;
pub mod projects;
pub mod rulings;
pub mod runs;
pub mod schedules;
pub mod spec;
pub mod tickets;
pub mod timeline;
pub mod workspaces;

#[cfg(test)]
mod test_support;

pub use backup::{
    BackupManifest, BackupOptions, BackupPreview, BackupRetentionPolicy, BackupSettings,
    BackupStore, VerifiedBackupHook, VerifiedBackupRecord, load_backup_settings,
};
#[cfg(test)]
mod timeline_scope_migration;

pub use capacity::SqliteCapacityStore;
pub use clone_guard::SqliteCloneGuardStore;
pub use comments::SqliteCommentStore;
pub use db::Database;
pub use deferrals::SqliteDeferralStore;
pub use dependencies::SqliteDependencyStore;
pub use dispatch::SqliteDispatchStore;
pub use error::StorageError;
pub use evidence::{SqliteEvidenceStore, content_hash};
pub use graph_proposals::SqliteGraphProposalStore;
pub use herdr::SqliteHerdrSettingsStore;
pub use idempotency::{RetentionPolicy, SqliteIdempotencyStore};
pub use initiatives::SqliteInitiativeStore;
pub use lanes::SqliteLaneStore;
pub use migrations::{
    AllowAllMigrations, Migration, MigrationReport, PendingMigration, PreMigrationHook,
};
pub use paths::{attachments_dir, backups_dir, config_file_name, database_path, managed_data_dir};
pub use plan::SqlitePlanStore;
pub use profiles::SqliteProfileStore;
pub use projects::SqliteProjectStore;
pub use rulings::SqliteRulingStore;
pub use runs::SqliteRunStore;
pub use schedules::SqliteScheduleStore;
pub use spec::SqliteSpecStore;
pub use tickets::SqliteTicketStore;
pub use timeline::{TimelineFilter, TimelineRow};
pub use workspaces::SqliteWorkspaceStore;
