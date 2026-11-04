# Sub-issues

Sub-issues follow Linear's parent/child model (see [linear-issue-model.md](./linear-issue-model.md)).

## Data model

- Each issue may have at most one parent (`parent_ticket_id`) and any number of
  children ordered by `sub_issue_sort`.
- Children are full issues with their own keys, states, labels, and dates.
- Cycles are rejected when setting a parent.

## MCP

- `save_issue` accepts `parent_id` on create and update; `null` clears the parent.
- `sort_order` on save sets child ordering (`sub_issue_sort`); `get_issue` lists children in that order.
- `get_issue` returns `parent` and `children` (identifier, title, state, sort).
- `list_issues` filters with `parent_id`.

When all children are in a **closed** state (Done, Cancelled, Superseded, or
Duplicate — matching Linear’s notion of completed/cancelled work), an **open**
parent is automatically moved to Done. Terminal parents are not updated again.

## UI

From the issue drawer, use **Add sub-issue** to create a child under the open
issue. The child inherits project and milestone from the parent; override them in
the create form if needed. The parent header shows progress (`N of M done`) where
**done** counts children in Done, Cancelled, Superseded, or Duplicate (same rule
as auto-close). Children list in manual sort order.
