//! SQLite implementation of the storage ports: connections,
//! forward-only migrations, and the append-only audit tables.

pub mod audit;
pub mod comments;
pub mod db;
pub mod deferrals;
pub mod error;
pub mod idempotency;
pub mod initiatives;
pub mod migrations;
pub mod paths;
pub mod rulings;
pub mod timeline;

#[cfg(test)]
mod test_support;

pub use comments::SqliteCommentStore;
pub use db::Database;
pub use deferrals::SqliteDeferralStore;
pub use error::StorageError;
pub use idempotency::{RetentionPolicy, SqliteIdempotencyStore};
pub use initiatives::SqliteInitiativeStore;
pub use migrations::{
    AllowAllMigrations, Migration, MigrationReport, PendingMigration, PreMigrationHook,
};
pub use paths::{database_path, managed_data_dir};
pub use rulings::SqliteRulingStore;
pub use timeline::{TimelineAppend, TimelineFilter, TimelineRow};
