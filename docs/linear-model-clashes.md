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
| Fixed transition table | Domain enforces allowed transitions and actor ownership | Legacy `state` column kept; per-project `issue_workflow_states` with Linear **types** seeded from the default names **plus Duplicate**. Operator `issue.save` applies **free transitions** via `IssueStore::apply_free_state` with history rows; UI drag still uses lifecycle rules until product changes. | None for operator path. |
| Board driven by `TicketState` | `board_group_for(TicketState)` | Workflow states carry `legacy_state`; board projection unchanged until UI reads types. **Remaining:** map by state **type** when custom names differ. | None. |
| Superseded vs duplicate | Superseded is terminal; duplicate was mapped to superseded legacy | **Duplicate** has its own workflow state and legacy `duplicate` value. `duplicate_of` on save moves relations to the canonical issue and transitions the duplicate issue to Duplicate. | None. |

## Priority and dates

| Clash | Kanban today | Resolution | Product decision |
|---|---|---|---|
| Urgent/High/Normal/Low text | No “none” level | `linear_priority` 0–4 added; legacy `priority` kept; mapped on migration. | None. |
| Task due in `due` column | Optional on tasks only | `due_date` on all tickets; migration `0054` copies task `due` when unset. Cleared with JSON `null` on `issue.save`. | Unset due dates stored as SQL NULL (omitted on read). |
| Estimates | Absent | `estimate` column added; unused. | None. |

## Relations

| Clash | Kanban today | Resolution | Product decision |
|---|---|---|---|
| `ticket_dependencies` only | Blocks edges + external blockers | `issue_relations` with `blocks`/`related`/`duplicate`; blocks copied from dependencies. `issue.get` lists blocks, blocked-by, and **related from both endpoints**. | None. |
| No related/duplicate | — | Related and duplicate-of implemented on operator save/get. | — |

## Comments

| Clash | Kanban today | Resolution | Product decision |
|---|---|---|---|
| Flat comments with revisions | No threads/resolution | `author` on `comments` (migration `0056`); `comment.list` returns author and timestamps. Thread columns exist; resolve UI not exposed. | None for list parity. |

## Projects and milestones

| Clash | Kanban today | Resolution | Product decision |
|---|---|---|---|
| Linear **Project** vs Kanban **Project** | Kanban Project = repo + seed | Kanban Project unchanged; Linear “project” maps to **Initiative** or nested naming **not done**. Milestones table is per Kanban project. | Sid: map Linear project to Initiative, Spec, or label? |
| Milestones | Absent | `issue_milestones` per project; optional FK on tickets. | None. |
| Name letter case | Label, label group, and milestone names were unique by exact spelling, so `WEB` could sit beside `web` | Unique per project ignoring letter case (Unicode full case folding), judged in `kanban-app` (`issue_names`) with migration `0066` triggers as an ASCII backstop. By-name lookups take the exact spelling, then the one case-insensitive match. Pairs stored before `0066` are kept, stay assignable by exact spelling, and make any other spelling an `invalid_request` instead of a guess. See [mcp-operator.md](./mcp-operator.md#label-and-milestone-names). | A case variant is `invalid_request` on `name` while an exact repeat stays `conflict`. |

## Keys and links

| Clash | Kanban today | Resolution | Product decision |
|---|---|---|---|
| `PREFIX-N` keys | Already per project | Unchanged; `IssueKey` + `kanban://issue/PREFIX-N` in domain; MCP save/get return link. | None. |

## MCP

| Clash | Kanban today | Resolution | Product decision |
|---|---|---|---|
| Run-scoped tools only | Capability-bound agent session | Installation operator session on loopback `/mcp/operator` (Bearer only) exposes Linear parity tools (`save_issue`, …). Agent surface unchanged. | Confirm operator auth = installation credential only. |
| Full ticket command surface | Hundreds of operator commands | Issue operator commands + `comment.list`; acceptance in `issue_mcp_acceptance.rs` and `issue_stage2_review.rs`. | None. |

## UI

| Clash | Kanban today | Resolution | Product decision |
|---|---|---|---|
| Ticket editors by kind | Separate Vue flows | **Remaining:** single issue editor with Type label + optional kind fields; surface `description` in drawer. | None. |
| State drag rules | Human task-only drag | **Remaining:** reflect free state changes where product allows. | Keep agent/human split on drag? |

## Execution stack (unchanged by design)

Dispatch, runs, lanes, landing, review, and graph approval still target tickets/issues by id. They observe new state fields; operator MCP bypasses lifecycle enforcement for state changes.

## Remaining work (product)

1. Assignee/delegate field and filters.
2. Comment threads and resolve in UI/MCP.
3. Board projection by workflow state **type** when names diverge from legacy defaults.
4. Linear “project” vs Kanban Project / Initiative mapping.
