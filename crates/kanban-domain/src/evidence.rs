//! Evidence items: managed files by content hash or repository
//! evidence by relative path and commit identity (KAN-S2-US4).

use std::fmt;

/// The identity of one evidence item. Assigned once by storage and
/// immutable afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EvidenceId(u64);

impl EvidenceId {
    /// Wrap a storage-assigned identity.
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    /// The underlying identity value.
    pub fn value(self) -> u64 {
        self.0
    }
}

impl fmt::Display for EvidenceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Whether evidence is a managed file or repository reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceKind {
    /// Bytes live in managed application data; SQLite holds the hash.
    ManagedFile,
    /// A relative path and commit identity; content is never copied.
    Repository,
}

/// Why evidence input was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceError {
    /// The content hash is not a lowercase SHA-256 hex digest.
    InvalidContentHash,
    /// The path is empty, absolute, or escapes its root.
    InvalidRelativePath,
    /// The commit identity is empty.
    BlankCommitIdentity,
    /// Managed-file evidence must carry a content hash.
    MissingContentHash,
    /// Repository evidence must carry a path and commit identity.
    MissingRepositoryFields,
    /// Managed-file evidence must not carry repository fields.
    UnexpectedRepositoryFields,
    /// Repository evidence must not carry a content hash.
    UnexpectedContentHash,
}

impl fmt::Display for EvidenceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidContentHash => {
                write!(f, "a content hash must be a lowercase SHA-256 hex digest")
            }
            Self::InvalidRelativePath => {
                write!(
                    f,
                    "a repository path must be relative and must not contain parent segments"
                )
            }
            Self::BlankCommitIdentity => write!(f, "a commit identity cannot be blank"),
            Self::MissingContentHash => {
                write!(f, "managed-file evidence requires a content hash")
            }
            Self::MissingRepositoryFields => {
                write!(
                    f,
                    "repository evidence requires a relative path and commit identity"
                )
            }
            Self::UnexpectedRepositoryFields => {
                write!(f, "managed-file evidence must not carry repository fields")
            }
            Self::UnexpectedContentHash => {
                write!(f, "repository evidence must not carry a content hash")
            }
        }
    }
}

impl std::error::Error for EvidenceError {}

/// A SHA-256 content hash for managed-file evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentHash(String);

impl ContentHash {
    /// Accept a lowercase SHA-256 hex digest.
    pub fn new(raw: &str) -> Result<Self, EvidenceError> {
        if raw.len() != 64
            || !raw
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        {
            return Err(EvidenceError::InvalidContentHash);
        }
        Ok(Self(raw.to_owned()))
    }

    /// The digest as stored and verified.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A repository-relative path for repository evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelativePath(String);

impl RelativePath {
    /// Accept a non-empty relative path without parent traversal.
    pub fn new(raw: &str) -> Result<Self, EvidenceError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('/') || trimmed.contains('\\') {
            return Err(EvidenceError::InvalidRelativePath);
        }
        for segment in trimmed.split('/') {
            if segment == ".." {
                return Err(EvidenceError::InvalidRelativePath);
            }
        }
        Ok(Self(trimmed.to_owned()))
    }

    /// The stored relative path.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A commit identity for repository evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitIdentity(String);

impl CommitIdentity {
    /// Accept a non-empty commit identity.
    pub fn new(raw: &str) -> Result<Self, EvidenceError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(EvidenceError::BlankCommitIdentity);
        }
        Ok(Self(trimmed.to_owned()))
    }

    /// The stored commit identity.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The durable fields of one evidence item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceShape {
    pub project_id: String,
    pub entity_kind: String,
    pub entity_id: String,
    pub kind: EvidenceKind,
    pub content_hash: Option<ContentHash>,
    pub relative_path: Option<RelativePath>,
    pub commit_identity: Option<CommitIdentity>,
}

/// One stored evidence item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceItem {
    id: EvidenceId,
    shape: EvidenceShape,
}

impl EvidenceItem {
    /// Restore a row read from storage.
    pub fn restore(id: EvidenceId, shape: EvidenceShape) -> Result<Self, EvidenceError> {
        let item = Self { id, shape };
        item.validate_shape()?;
        Ok(item)
    }

    /// The immutable identity.
    pub fn id(&self) -> EvidenceId {
        self.id
    }

    /// The owning Project.
    pub fn project_id(&self) -> &str {
        &self.shape.project_id
    }

    /// The entity kind this evidence is attached to.
    pub fn entity_kind(&self) -> &str {
        &self.shape.entity_kind
    }

    /// The entity identity this evidence is attached to.
    pub fn entity_id(&self) -> &str {
        &self.shape.entity_id
    }

    /// Whether this is managed-file or repository evidence.
    pub fn kind(&self) -> EvidenceKind {
        self.shape.kind
    }

    /// The content hash when this is managed-file evidence.
    pub fn content_hash(&self) -> Option<&ContentHash> {
        self.shape.content_hash.as_ref()
    }

    /// The relative path when this is repository evidence.
    pub fn relative_path(&self) -> Option<&RelativePath> {
        self.shape.relative_path.as_ref()
    }

    /// The commit identity when this is repository evidence.
    pub fn commit_identity(&self) -> Option<&CommitIdentity> {
        self.shape.commit_identity.as_ref()
    }

    fn validate_shape(&self) -> Result<(), EvidenceError> {
        match self.shape.kind {
            EvidenceKind::ManagedFile => {
                if self.shape.content_hash.is_none() {
                    return Err(EvidenceError::MissingContentHash);
                }
                if self.shape.relative_path.is_some() || self.shape.commit_identity.is_some() {
                    return Err(EvidenceError::UnexpectedRepositoryFields);
                }
            }
            EvidenceKind::Repository => {
                if self.shape.relative_path.is_none() || self.shape.commit_identity.is_none() {
                    return Err(EvidenceError::MissingRepositoryFields);
                }
                if self.shape.content_hash.is_some() {
                    return Err(EvidenceError::UnexpectedContentHash);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CommitIdentity, ContentHash, EvidenceError, EvidenceId, EvidenceItem, EvidenceKind,
        EvidenceShape, RelativePath,
    };

    #[test]
    fn a_content_hash_accepts_a_lowercase_sha256_digest() {
        let digest = "a".repeat(64);
        let hash = ContentHash::new(&digest).expect("the digest validates");
        assert_eq!(hash.as_str(), digest);
    }

    #[test]
    fn a_content_hash_refuses_uppercase_or_short_values() {
        assert!(matches!(
            ContentHash::new(&"A".repeat(64)),
            Err(EvidenceError::InvalidContentHash)
        ));
        assert!(matches!(
            ContentHash::new("abc"),
            Err(EvidenceError::InvalidContentHash)
        ));
    }

    #[test]
    fn a_relative_path_refuses_absolute_or_parent_segments() {
        assert!(matches!(
            RelativePath::new("../secret.txt"),
            Err(EvidenceError::InvalidRelativePath)
        ));
        assert!(matches!(
            RelativePath::new("/absolute.txt"),
            Err(EvidenceError::InvalidRelativePath)
        ));
        let path = RelativePath::new("docs/spec.md").expect("the path validates");
        assert_eq!(path.as_str(), "docs/spec.md");
    }

    #[test]
    fn managed_file_evidence_requires_only_a_hash() {
        let hash = ContentHash::new(&"b".repeat(64)).expect("the digest validates");
        let item = EvidenceItem::restore(
            EvidenceId::new(1),
            EvidenceShape {
                project_id: "kan-p1".to_owned(),
                entity_kind: "ticket".to_owned(),
                entity_id: "kan-t10".to_owned(),
                kind: EvidenceKind::ManagedFile,
                content_hash: Some(hash),
                relative_path: None,
                commit_identity: None,
            },
        )
        .expect("the shape validates");
        assert_eq!(item.kind(), EvidenceKind::ManagedFile);
    }

    #[test]
    fn repository_evidence_requires_path_and_commit_only() {
        let path = RelativePath::new("src/main.rs").expect("the path validates");
        let commit = CommitIdentity::new("deadbeef").expect("the commit validates");
        let item = EvidenceItem::restore(
            EvidenceId::new(2),
            EvidenceShape {
                project_id: "kan-p1".to_owned(),
                entity_kind: "ticket".to_owned(),
                entity_id: "kan-t10".to_owned(),
                kind: EvidenceKind::Repository,
                content_hash: None,
                relative_path: Some(path),
                commit_identity: Some(commit),
            },
        )
        .expect("the shape validates");
        assert_eq!(item.kind(), EvidenceKind::Repository);
    }
}
