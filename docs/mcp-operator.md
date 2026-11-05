# Installation operator MCP

The operator surface is available on loopback at `/mcp/operator` with Bearer
installation credentials. It exposes Linear-shaped issue tools for Sid's shared
computer workflows.

## Tools

MCP tool names map to Core operations via `packages/contracts/src/client.ts`.
The table lists the **MCP name** (what clients call) and the **Core operation**.

| MCP name | Core operation | Purpose |
| --- | --- | --- |
| `save_issue` | `issue.save` | Create or update title, description, state, priority, due date, milestone, labels, parent (in any project), duplicate-of, relations, and blocked-by |
| `get_issue` | `issue.get` | Read one issue by `TEAM-N` key with relations, sub-issue progress, state history, lifecycle timestamps, version, and timestamps |
| `list_issues` | `issue.list` | Filter by project/team, state, label, milestone, text query, parent (children in every project unless a project filter narrows them), and `updated_since`; paginate with `cursor` and `limit` (default 50) |
| `list_issue_statuses` | `issue.list_workflow_states` | Workflow states with Linear types for a project |
| `list_issue_labels` | `issue.list_labels` | Labels and groups (including single-select groups), including retired labels |
| `list_milestones` | `issue.list_milestones` | Milestones for a project with issue progress counts |
| `list_projects` | `issue.list_projects` | Registered projects (teams) |
| `create_milestone` | `issue.create_milestone` | Create a milestone on a project |
| `create_label` | `issue.create_label` | Create a label (and optional group) on a project |
| `retire_label` | `issue.retire_label` | Retire (archive) a label on a project |
| `update_project_settings` | `issue.update_project_settings` | Per-project settings such as `auto_close_sub_issues` (default off) |
| `comment_create` | `comment.create` | Post a comment (optional thread parent); body stored without trimming trailing newlines |
| `comment_edit` | `comment.edit` | Append a revision to an existing comment |
| `comment_revisions` | `comment.revisions` | Read comment history |
| `list_comments` | `comment.list` | List comments on an issue (oldest first, cursor paging), including resolve state |
| `resolve_comment` | `comment.resolve` | Resolve or clear resolution on a comment thread |

Sub-issues may sit in a different project from their parent; see
[sub-issues.md](./sub-issues.md) for how the project and milestone default and
how reads behave across projects.

Agent-scoped MCP sessions keep the existing capability-bound tool set; operator
tools are installation-authenticated only.

## Acceptance

Automated coverage lives in:

- `crates/kanban-app/tests/issue_mcp_acceptance.rs` — scenarios from `docs/mcp-acceptance-tests.md`
- `crates/kanban-app/tests/issue_lin_acceptance.rs` — Lin L1–L10 parity checks (fresh DB)
- `crates/kanban-storage/src/linear_issue_migration.rs` — migrated schema-52 fixture (`migration_52_to_54_preserves_…`)

The doc/tool-name pairing is checked in `l10_description_patch_in_place_and_mcp_doc_matches_catalog`.
