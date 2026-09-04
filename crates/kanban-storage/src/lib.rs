//! SQLite implementation of the storage ports: connections,
//! forward-only migrations, and the append-only audit tables.

pub mod db;
pub mod error;
pub mod migrations;
pub mod paths;

#[cfg(test)]
mod test_support;

pub use db::Database;
pub use error::StorageError;
pub use migrations::{
    AllowAllMigrations, Migration, MigrationReport, PendingMigration, PreMigrationHook,
};
pub use paths::{database_path, managed_data_dir};
