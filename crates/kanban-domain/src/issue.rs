//! Linear issue vocabulary: workflow state types, priority levels, and
//! stable issue keys used alongside the legacy Ticket lifecycle.

use crate::project::ProjectCode;
use crate::ticket::TicketNumber;

/// Linear workflow state type (fixed enum).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WorkflowStateType {
    Backlog,
    Unstarted,
    Started,
    Completed,
    Canceled,
    Duplicate,
}

impl WorkflowStateType {
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Backlog => "backlog",
            Self::Unstarted => "unstarted",
            Self::Started => "started",
            Self::Completed => "completed",
            Self::Canceled => "canceled",
            Self::Duplicate => "duplicate",
        }
    }

    pub fn parse(stored: &str) -> Option<Self> {
        Some(match stored {
            "backlog" => Self::Backlog,
            "unstarted" => Self::Unstarted,
            "started" => Self::Started,
            "completed" => Self::Completed,
            "canceled" => Self::Canceled,
            "duplicate" => Self::Duplicate,
            _ => return None,
        })
    }
}

/// Linear priority: 0 None through 4 Low.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LinearPriority(u8);

impl LinearPriority {
    pub fn new(value: u8) -> Option<Self> {
        (value <= 4).then_some(Self(value))
    }

    pub fn value(self) -> u8 {
        self.0
    }
}

/// Stable external key `PREFIX-123`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IssueKey {
    code: ProjectCode,
    number: TicketNumber,
}

impl IssueKey {
    pub fn new(code: ProjectCode, number: TicketNumber) -> Self {
        Self { code, number }
    }

    pub fn display(&self) -> String {
        format!("{}-{}", self.code.as_str(), self.number.value())
    }

    pub fn project_code(&self) -> &ProjectCode {
        &self.code
    }

    pub fn number(&self) -> TicketNumber {
        self.number
    }

    pub fn deep_link(&self) -> String {
        format!("kanban://issue/{}", self.display())
    }

    pub fn parse(identifier: &str) -> Option<Self> {
        let (code, number) = identifier.rsplit_once('-')?;
        let code = ProjectCode::new(code).ok()?;
        let number = TicketNumber::new(number.parse().ok()?).ok()?;
        Some(Self { code, number })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_key_formats_identifier_and_deep_link() {
        let key = IssueKey::new(
            ProjectCode::new("CORE").unwrap(),
            TicketNumber::new(17).unwrap(),
        );
        assert_eq!(key.display(), "CORE-17");
        assert_eq!(key.deep_link(), "kanban://issue/CORE-17");
        assert_eq!(IssueKey::parse("CORE-17"), Some(key));
    }
}
