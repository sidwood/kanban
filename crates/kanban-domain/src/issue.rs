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

    /// The Linear priority a legacy Ticket priority maps to, matching
    /// the Linear migration's rewrite of stored rows.
    pub fn of_legacy(priority: crate::ticket::Priority) -> Self {
        Self(match priority {
            crate::ticket::Priority::Urgent => 1,
            crate::ticket::Priority::High => 2,
            crate::ticket::Priority::Normal => 3,
            crate::ticket::Priority::Low => 4,
        })
    }
}

/// The scheme and host every issue deep link starts with.
const DEEP_LINK_PREFIX: &str = "kanban://issue/";

/// The longest link [`IssueKey::deep_link`] can mint: the prefix, an
/// eight-character code (`[A-Z][A-Z0-9]{1,7}`), the hyphen, and the
/// twenty digits of the largest number. Longer text is refused before
/// it is read.
const DEEP_LINK_MAX_LEN: usize = DEEP_LINK_PREFIX.len() + 8 + 1 + 20;

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
        format!("{DEEP_LINK_PREFIX}{}", self.display())
    }

    /// The key a deep link names, when the link is exactly one
    /// [`Self::deep_link`] mints. Anything else names nothing: another
    /// scheme or host, a key spelt otherwise (lowercase, zero-padded,
    /// signed), a further path segment, a query, a fragment, or text
    /// longer than any minted link.
    pub fn from_deep_link(link: &str) -> Option<Self> {
        if link.len() > DEEP_LINK_MAX_LEN {
            return None;
        }
        let key = Self::parse(link.strip_prefix(DEEP_LINK_PREFIX)?)?;
        (key.deep_link() == link).then_some(key)
    }

    pub fn parse(identifier: &str) -> Option<Self> {
        let (code, number) = identifier.rsplit_once('-')?;
        let code = ProjectCode::new(&code.to_ascii_uppercase()).ok()?;
        let number = TicketNumber::new(number.parse().ok()?).ok()?;
        Some(Self { code, number })
    }
}

/// Side effects when an issue enters a workflow state type.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IssueStateTimestamps {
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub canceled_at: Option<String>,
}

impl IssueStateTimestamps {
    /// Stamp Linear lifecycle timestamps when `state_type` is entered.
    pub fn stamp_entry(&mut self, state_type: WorkflowStateType, at: &str) {
        match state_type {
            WorkflowStateType::Started => {
                if self.started_at.is_none() {
                    self.started_at = Some(at.to_owned());
                }
            }
            WorkflowStateType::Completed => {
                self.completed_at = Some(at.to_owned());
                self.canceled_at = None;
            }
            WorkflowStateType::Canceled | WorkflowStateType::Duplicate => {
                self.canceled_at = Some(at.to_owned());
                self.completed_at = None;
            }
            WorkflowStateType::Backlog | WorkflowStateType::Unstarted => {}
        }
    }

    /// Clear terminal timestamps when an issue reopens from completed/canceled.
    pub fn clear_terminal_on_reopen(&mut self, to_state: WorkflowStateType) {
        if matches!(
            to_state,
            WorkflowStateType::Started | WorkflowStateType::Backlog | WorkflowStateType::Unstarted
        ) {
            self.completed_at = None;
            self.canceled_at = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_timestamps_follow_linear_type_rules() {
        let mut stamps = IssueStateTimestamps::default();
        stamps.stamp_entry(WorkflowStateType::Started, "2026-11-03T12:00:00Z");
        stamps.stamp_entry(WorkflowStateType::Started, "2026-11-03T13:00:00Z");
        assert_eq!(stamps.started_at.as_deref(), Some("2026-11-03T12:00:00Z"));
        stamps.stamp_entry(WorkflowStateType::Completed, "2026-11-03T14:00:00Z");
        assert_eq!(stamps.completed_at.as_deref(), Some("2026-11-03T14:00:00Z"));
    }

    #[test]
    fn entering_a_terminal_state_clears_the_opposite_stamp() {
        let mut stamps = IssueStateTimestamps::default();
        stamps.stamp_entry(WorkflowStateType::Completed, "2026-11-03T14:00:00Z");
        assert_eq!(stamps.completed_at.as_deref(), Some("2026-11-03T14:00:00Z"));
        stamps.stamp_entry(WorkflowStateType::Canceled, "2026-11-03T15:00:00Z");
        assert_eq!(
            stamps.canceled_at.as_deref(),
            Some("2026-11-03T15:00:00Z"),
            "entering canceled stamps the canceled time"
        );
        assert_eq!(
            stamps.completed_at, None,
            "a direct completed to canceled move drops the completed stamp"
        );
        stamps.stamp_entry(WorkflowStateType::Completed, "2026-11-03T16:00:00Z");
        assert_eq!(
            stamps.completed_at.as_deref(),
            Some("2026-11-03T16:00:00Z"),
            "entering completed stamps the completed time"
        );
        assert_eq!(
            stamps.canceled_at, None,
            "a direct canceled to completed move drops the canceled stamp"
        );
        stamps.stamp_entry(WorkflowStateType::Duplicate, "2026-11-03T17:00:00Z");
        assert_eq!(stamps.canceled_at.as_deref(), Some("2026-11-03T17:00:00Z"));
        assert_eq!(
            stamps.completed_at, None,
            "a direct completed to duplicate move drops the completed stamp"
        );
    }

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

    fn key(code: &str, number: u64) -> IssueKey {
        IssueKey::new(
            ProjectCode::new(code).expect("the fixture code is well formed"),
            TicketNumber::new(number).expect("the fixture number is positive"),
        )
    }

    fn refused(links: &[&str]) {
        for link in links {
            assert_eq!(
                IssueKey::from_deep_link(link),
                None,
                "`{link}` names no issue"
            );
        }
    }

    #[test]
    fn a_minted_deep_link_names_its_issue() {
        assert_eq!(
            IssueKey::from_deep_link("kanban://issue/CORE-12"),
            Some(key("CORE", 12))
        );
        for minted in [key("AB", 1), key("CORE", 17), key("A1B2C3D4", u64::MAX)] {
            assert_eq!(
                IssueKey::from_deep_link(&minted.deep_link()),
                Some(minted.clone()),
                "the link {} mints names it",
                minted.display()
            );
        }
    }

    #[test]
    fn a_deep_link_spells_its_key_exactly_as_minted() {
        refused(&[
            "kanban://issue/core-12",
            "kanban://issue/Core-12",
            "kanban://issue/CORE-012",
            "kanban://issue/CORE-+12",
            "kanban://issue/CORE-0",
            "kanban://issue/CORE-1.5",
            "kanban://issue/CORE_12",
            "kanban://issue/CORE%2D12",
            "kanban://issue/ CORE-12",
            "kanban://issue/CORE-12 ",
            "kanban://issue/CORE-18446744073709551616",
        ]);
    }

    #[test]
    fn a_deep_link_names_a_project_code_and_a_number() {
        refused(&[
            "kanban://issue/KAN-1",
            "kanban://issue/C-1",
            "kanban://issue/ABCDEFGHI-1",
            "kanban://issue/1CORE-1",
            "kanban://issue/CORE",
            "kanban://issue/12",
            "kanban://issue/CORE-12-3",
        ]);
    }

    #[test]
    fn a_deep_link_with_more_than_the_key_names_no_issue() {
        refused(&[
            "kanban://issue/CORE-12/",
            "kanban://issue/CORE-12/comments",
            "kanban://issue//CORE-12",
            "kanban://issue/x/../CORE-12",
            "kanban://issue/CORE-12?",
            "kanban://issue/CORE-12?tab=history",
            "kanban://issue/CORE-12?token=secret",
            "kanban://issue/CORE-12#comments",
        ]);
    }

    #[test]
    fn a_link_of_another_scheme_names_no_issue() {
        refused(&[
            "https://issue/CORE-12",
            "http://kanban/issue/CORE-12",
            "file:///issue/CORE-12",
            "javascript:alert(1)",
            "KANBAN://issue/CORE-12",
            "kanban-dev://issue/CORE-12",
            "kanban:issue/CORE-12",
            "kanban:/issue/CORE-12",
        ]);
    }

    #[test]
    fn a_deep_link_to_another_host_names_no_issue() {
        refused(&[
            "kanban://issues/CORE-12",
            "kanban://ticket/CORE-12",
            "kanban://Issue/CORE-12",
            "kanban://ISSUE/CORE-12",
            "kanban://issue.example/CORE-12",
            "kanban://user@issue/CORE-12",
            "kanban://issue:80/CORE-12",
            "kanban:///issue/CORE-12",
            "kanban:///CORE-12",
        ]);
    }

    #[test]
    fn a_deep_link_with_no_key_names_no_issue() {
        refused(&[
            "",
            "kanban://",
            "kanban://issue",
            "kanban://issue/",
            "kanban://issue/-",
            "kanban://issue/-12",
            "kanban://issue/CORE-",
        ]);
    }

    #[test]
    fn a_deep_link_longer_than_any_minted_one_is_refused() {
        let longest = key("ABCDEFGH", u64::MAX);
        assert_eq!(
            IssueKey::from_deep_link(&longest.deep_link()),
            Some(longest.clone()),
            "the longest link the core can mint still names its issue"
        );
        for link in [
            format!("{}0", longest.deep_link()),
            format!("kanban://issue/CORE-{}", "1".repeat(10_000)),
            format!("kanban://issue/{}-1", "A".repeat(1 << 20)),
            format!("kanban://issue/CORE-12{}", "/".repeat(1 << 20)),
        ] {
            assert_eq!(
                IssueKey::from_deep_link(&link),
                None,
                "a {}-byte link names no issue",
                link.len()
            );
        }
    }
}
