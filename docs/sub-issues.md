# Sub-issues

Sub-issues follow Linear's parent/child model (see [linear-issue-model.md](./linear-issue-model.md)).

## Data model

- Each issue may have at most one parent (`parent_ticket_id`) and any number of
  children ordered by `sub_issue_sort`.
- Children are full issues with their own keys, states, labels, and dates.
- A child may belong to a different project from its parent, as in Linear. It
  keeps its own project's key (`EDGE-4` under `CORE-12`), workflow states,
  labels, and milestone.
- Cycles are rejected when setting a parent, including cycles that pass through
  another project.

## MCP

- `save_issue` accepts `parent_id` (issue key or numeric ticket id) on create
  and update; `null` clears the parent.
- On create, a sub-issue takes its parent's project when `project_id` is
  omitted; an explicit `project_id` overrides it.
- A new sub-issue in its parent's project inherits the parent's milestone
  unless the request names one (`milestone_id`, or `null` for none). A
  milestone belongs to one project, so a sub-issue created in another project
  starts with no milestone, and naming a milestone of the parent's project for
  it is refused (`milestone_id`) with nothing created.
- Re-parenting (`parent_id` on update) may move an issue under a parent in any
  project. The issue keeps its own project and milestone.
- `sort_order` on save sets child ordering (`sub_issue_sort`); `get_issue` lists children in that order.
- `get_issue` returns `parent` and `children` (identifier, title, state, sort)
  from every project. Each child's `state` is the name in its own project's
  workflow.
- The `sub_issues` progress count (`done` of `total`) counts children in every
  project; a child is done when its state type is completed, canceled, or
  duplicate.
- `list_issues` filters with `parent_id` (numeric ticket id or issue key) and
  returns children in every project; add `project_id` or `team_id` to narrow
  them to one project.

When all children are in a **closed** state (Done, Cancelled, Superseded, or
Duplicate — matching Linear’s notion of completed/cancelled work), an **open**
parent can be moved to Done automatically when **`auto_close_sub_issues`** is
enabled for the project (`update_project_settings`; **default off**). The
setting is read from the **parent's** project, so children in other projects
count, and their own projects' settings do not close the parent. Terminal
parents are not updated again.

The opposite direction is Linear's "auto-close sub-issues": when
**`close_sub_issues_with_parent`** is enabled for the parent's project
(`update_project_settings`; **default off**), a parent entering a closed state
closes its **open** sub-issues:

| Parent enters | Open sub-issues move to |
| --- | --- |
| Done (completed) | Done (the first completed state of the sub-issue's project) |
| Cancelled or Superseded (canceled) | Cancelled (the first canceled state) |
| Duplicate | Cancelled; a sub-issue duplicates nothing |

- Sub-issues that are already closed keep their state, so finished work stays
  Done under a cancelled parent.
- Each move is an ordinary state move: it ends the sub-issue's current history
  row, appends a new one, stamps `completed_at` or `canceled_at`, bumps the
  version, and records a timeline event naming the parent.
- The rule cascades: a closed sub-issue closes its own open sub-issues when its
  project has the setting on.
- It runs whichever way the parent closes: `save_issue` state moves,
  `duplicate_of`, parent auto-close, and legacy ticket moves such as cancel.
- Reopening the parent leaves its sub-issues closed.
- A sub-issue with one of its own runs still executing stays open. The
  refusal writes nothing, the same as any refused move, and that child's
  own sub-issues are not closed by this cascade.
- With both settings on, every issue enters its closed state once: this rule
  only closes open sub-issues and parent auto-close only closes open parents.

`list_projects` reads both settings back for every project, and
`update_project_settings` returns both after any change.

## UI

From the issue drawer, use **Add sub-issue** to create a child under the open
issue. The child inherits project and milestone from the parent; choose another
project in the create form to place the child there (it then starts with no
milestone). The parent header shows progress (`N of M done`) where
**done** counts children in Done, Cancelled, Superseded, or Duplicate (same rule
as auto-close). Children list in manual sort order, each under its own key, so a
child from another project shows that project's code; choosing it opens it in
the drawer.
