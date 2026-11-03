# Linear model clashes with Kanban

This document records every deliberate clash between Sid's Linear workspace
model and Kanban's existing ticket-centric design, how this branch resolves
it, and what still needs a product decision.

## Issue shape

| Clash | Kanban today | Resolution on `linear-issue-model` | Product decision |
|---|---|---|---|
| Three ticket kinds with enforced fields | SQL CHECK and triggers require kind-specific columns | Migration `0053` adds labels and Linear fields; kind columns and triggers remain for compatibility. **Follow-up:** migration `0054` must relax CHECK/triggers so kind fields are optional (brief). | None if optional fields ship in `0054`. |
| Single issue type | `implementation`, `bug`, `task` | Per-project **Type** label group (single-select) seeded from existing `kind`. | None. |
| Assignee / delegate | No assignee field | Not stored yet; MCP acceptance tests mention assignee filters. **Remaining:** assignee column or profile link. | Sid: is assignee a local profile name, free text, or omitted for v1? |

## Workflow

| Clash | Kanban today | Resolution | Product decision |
|---|---|---|---|
| Fixed transition table | Domain enforces allowed transitions and actor ownership | Legacy `state` column kept; per-project `issue_workflow_states` with Linear **types** seeded from the twelve names. **Remaining:** free transitions in domain/app; agents react without enforcing. | None once enforcement is removed in app layer. |
| Board driven by `TicketState` | `board_group_for(TicketState)` | Workflow states carry `legacy_state`; board projection unchanged until UI reads types. **Remaining:** map by state **type** when custom names differ. | None. |
| Superseded vs duplicate | Superseded is terminal; no duplicate merge | `Superseded` mapped to canceled **type**; duplicate relation + merge **not implemented** yet. | Confirm duplicate merge moves relations like Linear. |

## Priority and dates

| Clash | Kanban today | Resolution | Product decision |
|---|---|---|---|
| Urgent/High/Normal/Low text | No “none” level | `linear_priority` 0–4 added; legacy `priority` kept; mapped on migration. | None. |
| Task due in `due` column | Optional on tasks only | `due_date` on all tickets (default `2099-12-31` for backfill). **Remaining:** wire task `due` into `due_date` in migration. | Default for issues without a due date? |
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
| `PREFIX-N` keys | Already per project | Unchanged; `IssueKey` + `kanban://issue/PREFIX-N` in domain. **Remaining:** return link from MCP writes. | None. |

## MCP

| Clash | Kanban today | Resolution | Product decision |
|---|---|---|---|
| Run-scoped tools only | Capability-bound agent session | **Remaining:** operator adapter with installation auth + Linear tool surface (`mcp-acceptance-tests.md`). | Confirm operator auth = installation credential only. |
| Full ticket command surface | Hundreds of operator commands | Linear parity tools (`save_issue`, `list_issues`, …) **not wired** on this increment. | None. |

## UI

| Clash | Kanban today | Resolution | Product decision |
|---|---|---|---|
| Ticket editors by kind | Separate Vue flows | **Remaining:** single issue editor with Type label + optional kind fields. | None. |
| State drag rules | Human task-only drag | **Remaining:** reflect free state changes where product allows. | Keep agent/human split on drag? |

## Execution stack (unchanged by design)

Dispatch, runs, lanes, landing, review, and graph approval still target tickets/issues by id. They must observe new state fields without enforcing Linear transitions — **app wiring remaining**.

## Remaining work on this branch (honest)

1. Migration `0054`: relax ticket shape CHECK/triggers; merge task `due` into `due_date`.
2. Storage/app: load/save workflow states, labels, relations, state history with free transitions and timestamps.
3. Operator MCP tools + acceptance test harness.
4. UI: issue model in board, drawer, editors (no redesign).
5. Regenerate contracts / `mcp-tools.json` for operator tools.
6. Full `just check` on Linux after app/UI/MCP land.
