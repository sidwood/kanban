# Kanban MCP acceptance tests (Linear parity)

Owner: Lin. Run against Kanban's MCP on the shared computer once Dev's branch is up. Each test mirrors a real Linear workflow from `linear-issue-model.md`. Pass means the behaviour matches Linear.

1. **Create:** an issue with title, Markdown description (with a checklist), state, priority (0–4), labels, project, milestone and due date. The result returns the key (`TEAM-N`) and a link.
2. **Read:** get by key returns every field, the relations and the state history. Listing filters by team, project, state, label, milestone, assignee, text query and updated-since, with pagination.
3. **Update:** change the title, state, priority and due date (and clear the due date with null). Patch-edit the description without resending it. Add and remove labels without replacing the set. A single-select label group allows only one label from the group.
4. **State moves:** any state can move to any other. Timestamps are stamped by state type (started, completed, canceled), and each move is appended to the state history.
5. **Relations:** add and remove blocks, blocked by and related to. Both ends see the link. Mentioning a key in a description adds a "related to" link.
6. **Duplicate merge:** setting duplicate-of moves the issue to the Duplicate state, and its relations move onto the target.
7. **Split:** create two issues from one, link them, and retitle and re-home the original to another milestone.
8. **Comments:** post a long Markdown report word for word (from a file), list comments to confirm it landed, then edit it. Check threaded replies and resolve.
9. **Lookups:** list states (name plus type), labels (with groups and retired ones), projects and milestones (with progress).
10. **Errors:** an unknown key, a milestone from a different project, or a retired label gives a clear error and makes no change.
