// Deterministic card ordering (DR-LC-11): priority and readiness
// decide where a card sits, the minted number breaks ties, and
// nothing else does — there is no manual ordering, so card position
// is never a decision. The ranks mirror the priority and lifecycle
// vocabularies the domain owns; the number tiebreaker is the minted,
// immutable identity every Ticket already carries, which is what
// keeps relative order stable under reload.
import type { TicketPriority, TicketRecord, TicketState } from '@kanban/contracts'

// Urgent first, low last (CONTEXT.md): the priority is the
// operator's one ordering lever.
const PRIORITY_RANK: Record<TicketPriority, number> = {
  urgent: 0,
  high: 1,
  normal: 2,
  low: 3,
}

// The canonical lifecycle position. Inside a column that holds
// several states, the card closer to landing sits higher: ready above
// scheduled, landing above approved. Terminal states never reach the
// columns, so their rank is never read.
const READINESS_RANK: Record<TicketState, number> = {
  draft: 0,
  parked: 1,
  blocked: 2,
  scheduled: 3,
  ready: 4,
  active: 5,
  in_review: 6,
  approved: 7,
  landing: 8,
  done: 9,
  cancelled: -1,
  superseded: -1,
}

/** The order the board, register, and Done table render cards in. */
export function orderCards(cards: readonly TicketRecord[]): TicketRecord[] {
  return [...cards].sort(
    (a, b) =>
      PRIORITY_RANK[a.priority] - PRIORITY_RANK[b.priority] ||
      READINESS_RANK[b.state] - READINESS_RANK[a.state] ||
      a.number - b.number,
  )
}
