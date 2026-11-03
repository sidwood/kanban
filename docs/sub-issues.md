# Sub-issues

Sub-issues follow Linear's parent/child model (see `docs/linear-issue-model.md`
§5a).

## Data model

- Each issue may have at most one parent (`parent_ticket_id`) and any number of
  children ordered by `sub_issue_sort`.
- Children are full issues with their own keys, states, labels, and dates.
- Cycles are rejected when setting a parent.

## MCP

- `save_issue` accepts `parent_id` on create and update; `null` clears the parent.
- `get_issue` returns `parent` and `children` (identifier, title, state, sort).
- `list_issues` filters with `parent_id`.

## UI

From the issue drawer, use **Add sub-issue** to create a child under the open
issue. The child inherits project and milestone from the parent; override them in
the create form if needed. The parent header shows progress (`done/total`) and
lists children in manual order.
