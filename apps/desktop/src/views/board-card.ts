// The card and row vocabulary the board, register, and Done table
// render: labels, tones, and the shapes their tables receive. Only
// presentation lives here; every field comes from the Ticket record
// the generated client returned.
import type { TicketKind, TicketRecord, TicketState } from '@kanban/contracts'
import type { StatusTone } from '../components/StatusBadge.vue'
import type { BoardColumnId } from './board-layout'

export const KIND_LABELS: Record<TicketKind, string> = {
  implementation: 'Implementation Ticket',
  bug: 'Bug Ticket',
  task: 'Task Ticket',
}

export const STATUS_LABELS: Record<TicketState, string> = {
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
}

export const STATUS_TONES: Record<TicketState, StatusTone> = {
  draft: 'neutral',
  parked: 'neutral',
  blocked: 'caution',
  scheduled: 'progress',
  ready: 'progress',
  active: 'positive',
  in_review: 'progress',
  approved: 'positive',
  landing: 'progress',
  done: 'positive',
  cancelled: 'neutral',
  superseded: 'neutral',
}

/**
 * The card surface a status wears where it shares a column with
 * others. Tone is the vocabulary: a board spelling its own colours
 * would drift from the badge sitting on the same card.
 */
export function statusSurfaceClass(state: TicketState): string {
  switch (STATUS_TONES[state]) {
    case 'caution':
      return 'border-caution/50 bg-caution/8'
    case 'critical':
      return 'border-critical/50 bg-critical/8'
    case 'positive':
      return 'border-accent/50 bg-accent/8'
    case 'progress':
      return 'border-info/50 bg-info/8'
    default:
      return 'border-line bg-surface'
  }
}

// The line a card leads with: the slice an Implementation names, or
// the title a Bug or Task carries.
export function boardCardTitle(ticket: TicketRecord): string {
  return ticket.slice ?? ticket.title ?? 'Untitled Ticket'
}

/** The Ticket's global identity, `KAN-T12`, minted by its Project. */
export function boardCardNumber(ticket: TicketRecord, projectCode: string): string {
  return `${projectCode}-T${ticket.number}`
}

/** The timeline entity id the core stores for one Ticket. */
export function ticketTimelineId(projectCode: string, ticketNumber: number): string {
  return `${projectCode.toLowerCase()}-t${ticketNumber}`
}

export interface BoardRegisterRow {
  ticket: TicketRecord
  number: string
  title: string
  kindLabel: string
  statusLabel: string
  statusTone: StatusTone
  /** The columns a register move can name for this row. */
  moves: readonly { column: BoardColumnId; label: string }[]
}

export interface BoardRegisterColumn {
  id: BoardColumnId
  label: string
  subheading: string
  /** A column holding several states says which one each row is on. */
  showsStatus: boolean
  rows: readonly BoardRegisterRow[]
}
