// Linear workflow vocabulary for UI moves: the twelve default states
// seeded per project (migration 0053), mapped to legacy Ticket states
// the board columns still group by.
import type { IssueWorkflowStateRecord, TicketState } from '@kanban/contracts'

export const DEFAULT_WORKFLOW_STATE_NAMES: readonly string[] = [
  'Draft',
  'Parked',
  'Blocked',
  'Scheduled',
  'Ready',
  'Active',
  'In Review',
  'Approved',
  'Landing',
  'Done',
  'Cancelled',
  'Superseded',
  'Duplicate',
]

const LEGACY_BY_WORKFLOW_NAME: Record<string, TicketState> = {
  Draft: 'draft',
  Parked: 'parked',
  Blocked: 'blocked',
  Scheduled: 'scheduled',
  Ready: 'ready',
  Active: 'active',
  'In Review': 'in_review',
  Approved: 'approved',
  Landing: 'landing',
  Done: 'done',
  Cancelled: 'cancelled',
  Superseded: 'superseded',
  Duplicate: 'duplicate',
}

const WORKFLOW_NAME_BY_LEGACY: Record<TicketState, string> = {
  draft: 'Draft',
  parked: 'Parked',
  blocked: 'Blocked',
  scheduled: 'Scheduled',
  ready: 'Ready',
  active: 'Active',
  in_review: 'In Review',
  approved: 'Approved',
  landing: 'Landing',
  done: 'Done',
  cancelled: 'Cancelled',
  superseded: 'Superseded',
  duplicate: 'Duplicate',
}

export function defaultWorkflowStates(): IssueWorkflowStateRecord[] {
  return DEFAULT_WORKFLOW_STATE_NAMES.map((name) => ({
    name,
    state_type: 'backlog',
  }))
}

export function legacyStateToWorkflowName(state: TicketState): string {
  return WORKFLOW_NAME_BY_LEGACY[state]
}

export function workflowNameToLegacyState(name: string): TicketState | undefined {
  return LEGACY_BY_WORKFLOW_NAME[name]
}

export function workflowMoveTargets(
  currentLegacy: TicketState,
  states: readonly IssueWorkflowStateRecord[],
): readonly string[] {
  const names =
    states.length > 0 ? states.map((entry) => entry.name) : [...DEFAULT_WORKFLOW_STATE_NAMES]
  const current = legacyStateToWorkflowName(currentLegacy)
  return names.filter((name) => name !== current)
}
