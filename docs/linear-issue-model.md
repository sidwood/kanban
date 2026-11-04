# Linear issue model (operator parity)

Kanban exposes a Linear-shaped **issue** surface over existing tickets. One issue maps to one
ticket row plus issue metadata (workflow state, labels, relations, sub-issues, duplicates).

## Identity and fields

- **Identifier:** `{PROJECT_CODE}-{number}` (for example `CORE-12`).
- **State:** Named workflow states per project (Draft, Active, Done, Duplicate, and the rest of
  the default set). Operator `issue.save` applies **free transitions** by state name; history is
  recorded in `issue_state_history`.
- **Priority:** Linear scale 1–4 (urgent → low).
- **Due date, milestone, parent, duplicate-of:** Optional. Send JSON `null` on update to clear
  due date, milestone, or parent.
- **Relations:** `blocks`, `blocked-by`, and `related`. Related links are listed from both ends.
- **Duplicates:** Marking duplicate-of moves the issue to **Duplicate**, relinks relations onto
  the canonical issue, and drops self-edges.

## Sub-issues

- Parent/child links use `parent_id` on save.
- Child order uses `sort_order` (numeric); `issue.get` lists children in that order.
- When every child reaches a **closed** workflow type (Done, Cancelled, Superseded, or
  Duplicate), an **open** parent is auto-closed to Done. Parents already terminal are left
  unchanged.

## Comments

- `comment.list` returns author, body, created time, and edited time when applicable.
- Authors are stored on the comment row (backfilled from timeline on migration).

## MCP / operator names

See [mcp-operator.md](./mcp-operator.md) for the command and query catalogue.
