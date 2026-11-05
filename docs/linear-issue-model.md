# Linear issue model (operator parity)

Kanban exposes a Linear-shaped **issue** surface over existing tickets. One issue maps to one
ticket row plus issue metadata (workflow state, labels, relations, sub-issues, duplicates).

## Identity and fields

- **Identifier:** `{PROJECT_CODE}-{number}` (for example `CORE-12`).
- **State:** Kanban's own workflow state names per project (Draft, Parked, Blocked, Scheduled,
  Ready, Active, In Review, Approved, Landing, Done, Cancelled, Superseded, Duplicate). There
  are no Linear name aliases. Operator `issue.save` applies **free transitions** by state name;
  history is recorded in `issue_state_history`.
- **Priority:** Linear scale 1–4 (urgent → low).
- **Due date, milestone, parent, duplicate-of:** Optional. Send JSON `null` on update to clear
  due date, milestone, or parent.
- **Relations:** `blocks`, `blocked-by`, and `related`. Related links are listed from both ends.
- **Duplicates:** Marking duplicate-of moves the issue to **Duplicate**, relinks relations onto
  the canonical issue, and drops self-edges.

## Sub-issues

- Parent/child links use `parent_id` on save. A sub-issue may live in a different project from
  its parent. Omitting `project_id` on create inherits the parent's project; an explicit
  `project_id` overrides it.
- Child order uses `sort_order` (numeric); `issue.get` lists children in that order.
- Both sub-issue settings are **opt-in** and **default off**, consistent with
  [sub-issues.md](./sub-issues.md). Neither runs unless the relevant project's setting is on.
  `list_projects` reads both back.
- **`auto_close_sub_issues`:** when every child reaches a **closed** workflow type (Done,
  Cancelled, Superseded, or Duplicate), an **open** parent is auto-closed to Done. The
  parent's project setting decides. Parents already terminal are left unchanged. A parent with
  one of its own runs still executing stays open.
- **`close_sub_issues_with_parent`:** closing a parent closes its open sub-issues. A completed
  parent completes them; a canceled or duplicate parent cancels them. A sub-issue with one of
  its own runs still executing stays open, and that refusal writes nothing.

## Comments

- `comment.list` returns author, body, created time, and edited time when applicable.
- Authors are stored on the comment row. Migration keeps a timeline author where one was
  recorded.
- Only a comment with no recorded author lists as `Unknown author`; the stored author stays
  empty, and that name is reserved on create.

## MCP / operator names

See [mcp-operator.md](./mcp-operator.md) for the command and query catalogue. Operator writes
may omit the mutation context, including the version.
