# Installation operator MCP

The operator surface is available on loopback at `/mcp/operator` with Bearer
installation credentials. It exposes Linear-shaped issue tools for Sid's shared
computer workflows.

## Tools

| MCP name | Core operation | Purpose |
| --- | --- | --- |
| `save_issue` | `issue.save` | Create or update title, description, state, priority, due date, milestone, labels, parent, duplicate-of, and relations |
| `get_issue` | `issue.get` | Read one issue by `TEAM-N` key with relations, state history, version, and timestamps |
| `list_issues` | `issue.list` | Filter by project, state, label, text query, parent, and `updated_since`; paginate with `cursor` and `limit` (default 50) |
| `list_workflow_states` | `issue.list_workflow_states` | States with Linear types for a project |
| `list_labels` | `issue.list_labels` | Labels and groups, including retired labels |
| `list_milestones` | `issue.list_milestones` | Milestones for a project |
| `save_comment` | `comment.create` | Post a comment on a ticket |
| `edit_comment` | `comment.edit` | Append a revision to an existing comment |
| `list_comment_revisions` | `comment.revisions` | Read comment history |
| `list_comments` | `comment.list` | List comments on an issue (oldest first, cursor paging) |

Agent-scoped MCP sessions keep the existing capability-bound tool set; operator
tools are installation-authenticated only.

## Acceptance

Automated coverage lives in `crates/kanban-app/tests/issue_mcp_acceptance.rs`,
mirroring `docs/mcp-acceptance-tests.md`.
