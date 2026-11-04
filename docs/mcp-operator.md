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
| `list_projects` | `issue.list_projects` | Registered projects (teams): `id`, `code`, `name`, and both sub-issue settings, `auto_close_sub_issues` and `close_sub_issues_with_parent` |
| `create_milestone` | `issue.create_milestone` | Create a milestone on a project |
| `create_label` | `issue.create_label` | Create a label (and optional group) on a project |
| `retire_label` | `issue.retire_label` | Retire (archive) a label on a project |
| `update_project_settings` | `issue.update_project_settings` | Per-project sub-issue settings, both default off: `auto_close_sub_issues` closes a parent once its sub-issues close; `close_sub_issues_with_parent` closes open sub-issues when their parent closes. Returns both |
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

## Label and milestone names

Label names and milestone names are unique within their project ignoring
letter case, the way Linear matches them. Case follows Unicode full case
folding, so `WEB` and `web` are one name, as are `Straße` and `STRASSE`.

- `create_label` and `create_milestone` refuse a name that differs from a
  stored one only in letter case as `invalid_request` on `name`, quoting the
  stored spelling, and write nothing. An exact repeat is still `conflict` on
  `name`.
- `save_issue` (`add_labels`, `remove_labels`), `retire_label` (`name`), and
  the `group` of `create_label` find a stored name whatever case names it:
  the exact spelling first, then the one stored name that matches ignoring
  case. A name nothing answers to behaves as before: `add_labels` refuses it
  as unknown, `remove_labels` reports it not found, `retire_label` reports it not
  found, and a new `group` is created.
- `list_issues` filters by `label` ignoring case and matches every stored
  spelling.
- No tool renames a label or milestone yet. The database refuses a rename
  into a case variant (migration `0066`), so a future rename tool inherits
  the rule.

### Databases that already hold case variants

A database written before migration `0066` may hold two labels, milestones,
or label groups whose names differ only in letter case. Upgrading keeps every
such row as it is: nothing is renamed, merged, or dropped, and the migration
cannot fail on them.

- Both rows still list in `list_issue_labels` and `list_milestones`, issues
  keep what they hold, and milestone progress counts are unchanged.
- Each row stays assignable by its exact spelling. Milestones are assigned by
  `milestone_id`, which never depended on case.
- A spelling that matches several stored rows and none exactly (`Web`
  against `web` and `WEB`) is refused as `invalid_request` on the field that
  named it, quoting every candidate, rather than guessed.
- `list_issues` with `label: "web"` returns the holders of either spelling.
- No further spelling can be created.
- To stop using one spelling, retire it by its exact spelling. Issues that
  hold it keep it and new issues can only take the other. A retired row
  still counts when a lookup decides whether a spelling is ambiguous.

## Acceptance

Automated coverage lives in:

- `crates/kanban-app/tests/issue_mcp_acceptance.rs` — scenarios from `docs/mcp-acceptance-tests.md`
- `crates/kanban-app/tests/issue_lin_acceptance.rs` — Lin L1–L10 parity checks (fresh DB)
- `crates/kanban-app/tests/issue_name_case.rs` — label and milestone name case rules, including an upgrade over stored case-variant pairs
- `crates/kanban-storage/src/linear_issue_migration.rs` — migrated schema-52 fixture (`migration_52_to_54_preserves_…`)

The doc/tool-name pairing is checked in `l10_description_patch_in_place_and_mcp_doc_matches_catalog`.
