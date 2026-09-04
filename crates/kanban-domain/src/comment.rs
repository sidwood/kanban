//! The Comment aggregate: editable remarks with immutable revision
//! history (CONTEXT.md, DR-AE-02).

use std::fmt;

use crate::timeline::is_entity_kind;

/// The identity of one Comment. Assigned once by storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CommentId(u64);

impl CommentId {
    /// Wrap a storage-assigned identity.
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    /// The underlying identity value.
    pub fn value(self) -> u64 {
        self.0
    }
}

impl fmt::Display for CommentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Why comment text was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextError {
    /// The text holds nothing but whitespace.
    Blank,
}

impl fmt::Display for TextError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Blank => write!(f, "comment text cannot be blank"),
        }
    }
}

impl std::error::Error for TextError {}

/// Validated, trimmed comment text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentText(String);

impl CommentText {
    /// Accept any text that holds at least one non-whitespace character.
    pub fn new(raw: &str) -> Result<Self, TextError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(TextError::Blank);
        }
        Ok(Self(trimmed.to_owned()))
    }

    /// The trimmed text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Why a comment transition was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentError {
    /// The target entity kind is unknown.
    UnknownEntityKind,
}

impl fmt::Display for CommentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownEntityKind => write!(f, "the target entity kind is unknown"),
        }
    }
}

impl std::error::Error for CommentError {}

/// The timeline-visible entity a Comment attaches to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentTarget {
    kind: String,
    id: String,
}

impl CommentTarget {
    /// Parse and validate a target entity reference.
    pub fn new(kind: &str, id: &str) -> Result<Self, CommentError> {
        if !is_entity_kind(kind) {
            return Err(CommentError::UnknownEntityKind);
        }
        let trimmed_id = id.trim();
        if trimmed_id.is_empty() {
            return Err(CommentError::UnknownEntityKind);
        }
        Ok(Self {
            kind: kind.to_owned(),
            id: trimmed_id.to_owned(),
        })
    }

    /// The entity kind wire value.
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// The entity identity.
    pub fn id(&self) -> &str {
        &self.id
    }
}

/// One immutable revision of comment text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentRevision {
    number: u64,
    text: CommentText,
}

impl CommentRevision {
    /// The one-based revision number.
    pub fn number(&self) -> u64 {
        self.number
    }

    /// The revision text.
    pub fn text(&self) -> &CommentText {
        &self.text
    }

    /// Restore one revision from durable storage.
    pub fn restore(number: u64, text: CommentText) -> Self {
        Self { number, text }
    }
}

/// A Comment with append-only revision history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    id: CommentId,
    project_id: String,
    target: CommentTarget,
    revisions: Vec<CommentRevision>,
    version: u64,
}

impl Comment {
    /// Create a fresh Comment with its first revision at version 1.
    pub fn create(
        id: CommentId,
        project_id: &str,
        target: CommentTarget,
        text: CommentText,
    ) -> Self {
        Self {
            id,
            project_id: project_id.to_owned(),
            target,
            revisions: vec![CommentRevision { number: 1, text }],
            version: 1,
        }
    }

    /// Restore a Comment from durable storage.
    pub fn restore(
        id: CommentId,
        project_id: String,
        target: CommentTarget,
        revisions: Vec<CommentRevision>,
        version: u64,
    ) -> Self {
        Self {
            id,
            project_id,
            target,
            revisions,
            version,
        }
    }

    /// The immutable identity.
    pub fn id(&self) -> CommentId {
        self.id
    }

    /// The owning Project.
    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    /// The entity this Comment attaches to.
    pub fn target(&self) -> &CommentTarget {
        &self.target
    }

    /// The aggregate version for optimistic mutation checks.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Every revision, oldest first. Revisions are immutable once
    /// recorded.
    pub fn revisions(&self) -> &[CommentRevision] {
        &self.revisions
    }

    /// The current text: always the latest revision.
    pub fn current_text(&self) -> &CommentText {
        self.revisions
            .last()
            .map(|revision| &revision.text)
            .expect("a Comment always has at least one revision")
    }

    /// Append a new revision and bump the version.
    pub fn edit(&mut self, text: CommentText) -> Result<&CommentRevision, TextError> {
        let number = self
            .revisions
            .last()
            .map(|revision| revision.number + 1)
            .expect("a Comment always has at least one revision");
        let revision = CommentRevision { number, text };
        self.revisions.push(revision);
        self.version += 1;
        Ok(self.revisions.last().expect("the revision just landed"))
    }
}

#[cfg(test)]
mod tests {
    use super::{Comment, CommentId, CommentTarget, CommentText, TextError};

    fn target() -> CommentTarget {
        CommentTarget::new("ticket", "kan-t11").expect("the target validates")
    }

    fn text(value: &str) -> CommentText {
        CommentText::new(value).expect("the text validates")
    }

    #[test]
    fn creating_starts_at_revision_one_and_version_one() {
        let comment = Comment::create(CommentId::new(1), "kan", target(), text("first thought"));

        assert_eq!(comment.version(), 1);
        assert_eq!(comment.revisions().len(), 1);
        assert_eq!(comment.revisions()[0].number(), 1);
        assert_eq!(comment.current_text().as_str(), "first thought");
    }

    #[test]
    fn editing_appends_a_revision_and_bumps_the_version() {
        let mut comment =
            Comment::create(CommentId::new(1), "kan", target(), text("first thought"));

        comment
            .edit(text("corrected thought"))
            .expect("the edit applies");

        assert_eq!(comment.version(), 2);
        assert_eq!(comment.revisions().len(), 2);
        assert_eq!(comment.revisions()[1].number(), 2);
        assert_eq!(comment.current_text().as_str(), "corrected thought");
        assert_eq!(
            comment.revisions()[0].text().as_str(),
            "first thought",
            "earlier revisions stay intact"
        );
    }

    #[test]
    fn blank_text_is_refused() {
        let error = CommentText::new("   ").expect_err("blank text is refused");
        assert_eq!(error, TextError::Blank);
    }

    #[test]
    fn unknown_entity_kinds_are_refused() {
        let error = CommentTarget::new("ghost", "kan-t11").expect_err("unknown kinds are refused");
        assert_eq!(error.to_string(), "the target entity kind is unknown");
    }
}
