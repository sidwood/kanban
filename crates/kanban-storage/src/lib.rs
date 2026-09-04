//! SQLite implementation of the storage ports: connections,
//! forward-only migrations, and the append-only audit tables.

pub mod error;
pub mod paths;

pub use error::StorageError;
pub use paths::{database_path, managed_data_dir};
