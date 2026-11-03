# Linear model clashes with Kanban

This document records every deliberate clash between Sid's Linear workspace
model and Kanban's existing ticket-centric design, how this branch resolves
it, and what still needs a product decision.

## Issue shape

| Clash | Kanban today | Resolution on `linear-issue-model` | Product decision |
|---|---|---|---|
| Three ticket kinds with enforced fields | SQL CHECK and triggers require kind-specific columns | Migration `0053` adds labels and Linear fields; migration `0054` relaxes CHECK/triggers so kind fields are optional and merges task `due` into `due_date`. | None. |
| Single issue type | `implementation`, `bug`, `task` | Per-project **Type** label group (single-select) seeded from existing `kind`. | None. |
| Assignee / delegate | No assignee field | Not stored yet; MCP acceptance tests mention assignee filters. **Remaining:** assignee column or profile link. | Sid: is assignee a local profile name, free text, or omitted for v1? |

## Workflow

| Clash | Kanban today | Resolution | Product decision |
|---|---|---|---|
| Fixed transition table | Domain enforces allowed transitions and actor ownership | Legacy `state` column kept; per-project `issue_workflow_states` with Linear **types** seeded from the twelve names. Operator `issue.save` applies **free transitions** via `IssueStore::apply_free_state`; UI drag still uses lifecycle rules until product changes. | None for operator path. |
| Board driven by `TicketState` | `board_group_for(TicketState)` | Workflow states carry `legacy_state`; board projection unchanged until UI reads types. **Remaining:** map by state **type** when custom names differ. | None. |
| Superseded vs duplicate | Superseded is terminal; no duplicate merge | `Superseded` mapped to canceled **type**; duplicate relation + merge **not implemented** yet. | Confirm duplicate merge moves relations like Linear. |

## Priority and dates

| Clash | Kanban today | Resolution | Product decision |
|---|---|---|---|
| Urgent/High/Normal/Low text | No “none” level | `linear_priority` 0–4 added; legacy `priority` kept; mapped on migration. | None. |
| Task due in `due` column | Optional on tasks only | `due_date` on all tickets; migration `0054` copies task `due` when unset. | Default `2099-12-31` for issues without a due date. |
| Estimates | Absent | `estimate` column added; unused. | None. |

## Relations

| Clash | Kanban today | Resolution | Product decision |
|---|---|---|---|
| `ticket_dependencies` only | Blocks edges + external blockers | `issue_relations` with `blocks`/`related`/`duplicate`; blocks copied from dependencies. **Remaining:** blocked-by symmetry, related-from description mentions, duplicate merge. | None. |
| No related/duplicate | — | Schema only. | — |

## Comments

| Clash | Kanban today | Resolution | Product decision |
|---|---|---|---|
| Flat comments with revisions | No threads/resolution | `parent_comment_id`, `resolved_at` on `comments`. **Remaining:** app/MCP expose thread + resolve. | None. |

## Projects and milestones

| Clash | Kanban today | Resolution | Product decision |
|---|---|---|---|
| Linear **Project** vs Kanban **Project** | Kanban Project = repo + seed | Kanban Project unchanged; Linear “project” maps to **Initiative** or nested naming **not done**. Milestones table is per Kanban project. | Sid: map Linear project to Initiative, Spec, or label? |
| Milestones | Absent | `issue_milestones` per project; optional FK on tickets. | None. |

## Keys and links

| Clash | Kanban today | Resolution | Product decision |
|---|---|---|---|
| `PREFIX-N` keys | Already per project | Unchanged; `IssueKey` + `kanban://issue/PREFIX-N` in domain; MCP save/get return link. | None. |

## MCP

| Clash | Kanban today | Resolution | Product decision |
|---|---|---|---|
| Run-scoped tools only | Capability-bound agent session | Installation operator session on loopback `/mcp/operator` (Bearer only) exposes Linear parity tools (`save_issue`, …). Agent surface unchanged. | Confirm operator auth = installation credential only. |
| Full ticket command surface | Hundreds of operator commands | Six issue operator commands registered; acceptance harness in `issue_mcp_acceptance.rs` covers create/read/state/errors. **Remaining:** relations, comments, duplicate merge per `mcp-acceptance-tests.md`. | None. |

## UI

| Clash | Kanban today | Resolution | Product decision |
|---|---|---|---|
| Ticket editors by kind | Separate Vue flows | **Remaining:** single issue editor with Type label + optional kind fields; surface `description` in drawer. | None. |
| State drag rules | Human task-only drag | **Remaining:** reflect free state changes where product allows. | Keep agent/human split on drag? |

## Execution stack (unchanged by design)

Dispatch, runs, lanes, landing, review, and graph approval still target tickets/issues by id. They observe new state fields; operator MCP bypasses lifecycle enforcement for state changes.

## Remaining work on this branch (honest)

1. Relations, duplicate merge, description mentions, comment threads in app/MCP.
2. UI: issue fields in board, drawer, editors (no redesign).
3. Full `mcp-acceptance-tests.md` coverage beyond the current harness.
4. Board projection by workflow state **type** when names diverge from legacy twelve.
