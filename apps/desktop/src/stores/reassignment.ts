// The reassignment state (DR-DE-07, KAN-S4-US7), driven entirely
// through the generated client: `ticket.reassign` replaces a Ticket
// by creating a replacement Ticket under its kind's schema and
// superseding the original. The replacement is stated whole — the
// changed plan restated under its own kind's fields — and the
// superseded original leaves the active board keeping its history and
// its number, neither reused nor lost. The command guards on the open
// Ticket's aggregate version and reports a refusal, never swallowing
// it; on success the open Ticket becomes the replacement the command
// returned, predecessor reference and all.
import { defineStore } from 'pinia'
import { KanbanClient } from '@kanban/contracts'
import type {
  TaskMode,
  TaskSubtype,
  TicketCriterion,
  TicketKind,
  TicketPriority,
  TicketRecord,
} from '@kanban/contracts'
import { asApiError } from '../core/transport'
import type { ShellTransport } from '../core/transport'

// The replacement one reassignment states: the kind whose schema it
// carries, its priority, and exactly the fields that kind owns.
export interface TicketReplacementFields {
  kind: TicketKind
  priority: TicketPriority
  spec_id?: number
  title?: string
  actual_behaviour?: string
  reporter_evidence?: string
  slice?: string
  criteria?: TicketCriterion[]
  subtype?: TaskSubtype
  mode?: TaskMode
  completion?: string[]
  scheduled_for?: string
  due?: string
}

export const useReassignmentStore = defineStore('reassignment', {
  state: () => ({
    ticket: null as TicketRecord | null,
    error: null as string | null,
  }),
  actions: {
    // Open the Ticket being replaced so the reassignment guards on its
    // current version.
    async open(transport: ShellTransport, ticketId: number): Promise<void> {
      try {
        this.ticket = await new KanbanClient(transport).queryTicketGet({ ticket_id: ticketId })
        this.error = null
      } catch (failure) {
        this.error = asApiError(failure).message
      }
    },
    // Replace the open Ticket: the replacement is stated whole under
    // its kind's schema, and the open Ticket becomes the replacement
    // the command returns.
    async reassign(
      transport: ShellTransport,
      replacement: TicketReplacementFields,
    ): Promise<boolean> {
      const ticket = this.ticket
      if (ticket === null) {
        this.error = 'open a Ticket before reassigning it'
        return false
      }
      try {
        this.ticket = await new KanbanClient(transport).commandTicketReassign({
          mutation: { optimistic_version: ticket.version, idempotency_key: crypto.randomUUID() },
          ticket_id: ticket.id,
          ...(replacement.spec_id !== undefined ? { spec_id: replacement.spec_id } : {}),
          ...(replacement.title !== undefined ? { title: replacement.title } : {}),
          ...(replacement.actual_behaviour !== undefined
            ? { actual_behaviour: replacement.actual_behaviour }
            : {}),
          ...(replacement.reporter_evidence !== undefined
            ? { reporter_evidence: replacement.reporter_evidence }
            : {}),
          ...(replacement.slice !== undefined ? { slice: replacement.slice } : {}),
          ...(replacement.criteria !== undefined ? { criteria: replacement.criteria } : {}),
          ...(replacement.subtype !== undefined ? { subtype: replacement.subtype } : {}),
          ...(replacement.mode !== undefined ? { mode: replacement.mode } : {}),
          ...(replacement.completion !== undefined ? { completion: replacement.completion } : {}),
          ...(replacement.scheduled_for !== undefined
            ? { scheduled_for: replacement.scheduled_for }
            : {}),
          ...(replacement.due !== undefined ? { due: replacement.due } : {}),
          kind: replacement.kind,
          priority: replacement.priority,
        })
        this.error = null
      } catch (failure) {
        this.error = asApiError(failure).message
        return false
      }
      return true
    },
  },
})
