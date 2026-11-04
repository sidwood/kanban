// Linear issue chips for board cards: priority, labels, due date,
// milestone, assignee placeholder, and sub-issue progress. Legacy
// Ticket kind vocabulary is not shown on the face.
import type { IssueRecord, TicketPriority, TicketRecord } from '@kanban/contracts'
import type { StatusTone } from '../components/StatusBadge.vue'
import type { CardChip } from './board-chips'
import { chipSurfaceClass } from './board-chips'

const TICKET_TO_LINEAR: Record<TicketPriority, number> = {
  urgent: 1,
  high: 2,
  normal: 3,
  low: 4,
}

const LINEAR_PRIORITY_LABELS: Record<number, string> = {
  0: 'None',
  1: 'Urgent',
  2: 'High',
  3: 'Medium',
  4: 'Low',
}

const PRIORITY_TONES: Record<number, StatusTone> = {
  0: 'neutral',
  1: 'critical',
  2: 'caution',
  3: 'neutral',
  4: 'neutral',
}

function chip(
  kind: CardChip['kind'],
  label: string,
  value: string,
  tone?: StatusTone,
): CardChip {
  return tone === undefined ? { kind, label, value } : { kind, label, value, tone }
}

export interface LinearChipContext {
  issue?: IssueRecord | null
  milestoneName?: string | null
  assignee?: string | null
}

/** Chips for one board card under the Linear issue model. */
export function linearChipsFor(
  ticket: TicketRecord,
  context: LinearChipContext,
): readonly CardChip[] {
  const issue = context.issue ?? undefined
  const priority = issue?.priority ?? TICKET_TO_LINEAR[ticket.priority]
  const chips: CardChip[] = [
    chip('priority', 'Priority', LINEAR_PRIORITY_LABELS[priority] ?? 'Medium', PRIORITY_TONES[priority]),
  ]

  const workflowState = issue?.state ?? null
  if (workflowState) {
    chips.push(chip('lane', 'State', workflowState, 'progress'))
  }

  for (const label of issue?.labels ?? []) {
    chips.push({ ...chip('subtype', 'Label', label), key: `label:${label}` })
  }

  const due = issue ? issue.due_date : ticket.due ? ticket.due.slice(0, 10) : null
  if (due) {
    chips.push(chip('schedule', 'Due', due))
  }

  if (context.milestoneName) {
    chips.push(chip('spec', 'Milestone', context.milestoneName))
  }

  if (context.assignee) {
    chips.push(chip('executor', 'Assignee', context.assignee))
  }

  const progress = issue?.sub_issues
  if (progress && progress.total > 0) {
    chips.push(
      chip('progress', 'Sub-issues', `${progress.done}/${progress.total} done`, 'progress'),
    )
  }

  return chips
}

export { chipSurfaceClass }
