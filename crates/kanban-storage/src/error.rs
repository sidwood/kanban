//! The storage error type.

/// Everything that can go wrong inside the storage layer.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    /// The user home directory cannot be determined, so managed
    /// application data has no location.
    #[error("the user home directory cannot be determined")]
    HomeUnknown,
}

#[cfg(test)]
mod tests {
    use super::StorageError;

    #[test]
    fn home_unknown_renders_a_stable_message() {
        assert_eq!(
            StorageError::HomeUnknown.to_string(),
            "the user home directory cannot be determined"
        );
    }
}
