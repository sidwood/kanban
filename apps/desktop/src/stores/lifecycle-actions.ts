// The lifecycle action state, driven entirely through the generated
// client: the drag that serves Task Tickets alone, the named human
// commands — park, unpark, schedule, cancel, review decisions,
// prioritise, and edit — that serve every kind, and the audited
// emergency override recovery runs with a named operator and reason
// (KAN-S4-US6). Every action guards on the open Ticket's aggregate
// version and reports a refusal, never swallowing it; a drag refused
// as agent-owned arrives here as that explanation.
import { defineStore } from 'pinia'
import { KanbanClient } from '@kanban/contracts'
import type {
  TicketPriority,
  TicketRecord,
  TicketReviewDecision,
  TicketState,
} from '@kanban/contracts'
import { asApiError } from '../core/transport'
import type { ShellTransport } from '../core/transport'

// The fields one edit carries: exactly the field the kind owns.
export interface TicketEditFields {
  title?: string
  slice?: string
}

export const useLifecycleActionsStore = defineStore('lifecycle-actions', {
  state: () => ({
    ticket: null as TicketRecord | null,
    error: null as string | null,
  }),
  actions: {
    // Open one Ticket so every action guards on its current version.
    async open(transport: ShellTransport, ticketId: number): Promise<void> {
      try {
        this.ticket = await new KanbanClient(transport).queryTicketGet({ ticket_id: ticketId })
        this.error = null
      } catch (failure) {
        this.error = asApiError(failure).message
      }
    },
    // Drag the open Ticket to a legal target. Task Tickets answer the
    // drag; an Implementation or Bug drag comes back refused with the
    // agent-owned explanation.
    async transition(transport: ShellTransport, to: TicketState): Promise<boolean> {
      return this.act(transport, (client, ticketId, version) =>
        client.commandTicketTransition({
          mutation: { optimistic_version: version, idempotency_key: crypto.randomUUID() },
          ticket_id: ticketId,
          to,
        }),
      )
    },
    // Set aside work that has not started executing.
    async park(transport: ShellTransport): Promise<boolean> {
      return this.act(transport, (client, ticketId, version) =>
        client.commandTicketPark({
          mutation: { optimistic_version: version, idempotency_key: crypto.randomUUID() },
          ticket_id: ticketId,
        }),
      )
    },
    // Return parked work to circulation.
    async unpark(transport: ShellTransport): Promise<boolean> {
      return this.act(transport, (client, ticketId, version) =>
        client.commandTicketUnpark({
          mutation: { optimistic_version: version, idempotency_key: crypto.randomUUID() },
          ticket_id: ticketId,
        }),
      )
    },
    // Hold qualified work until its activation.
    async schedule(transport: ShellTransport): Promise<boolean> {
      return this.act(transport, (client, ticketId, version) =>
        client.commandTicketSchedule({
          mutation: { optimistic_version: version, idempotency_key: crypto.randomUUID() },
          ticket_id: ticketId,
        }),
      )
    },
    // End the Ticket. Cancelled is terminal.
    async cancel(transport: ShellTransport): Promise<boolean> {
      return this.act(transport, (client, ticketId, version) =>
        client.commandTicketCancel({
          mutation: { optimistic_version: version, idempotency_key: crypto.randomUUID() },
          ticket_id: ticketId,
        }),
      )
    },
    // Record one explicit review decision.
    async review(
      transport: ShellTransport,
      decision: TicketReviewDecision,
    ): Promise<boolean> {
      return this.act(transport, (client, ticketId, version) =>
        client.commandTicketReview({
          mutation: { optimistic_version: version, idempotency_key: crypto.randomUUID() },
          ticket_id: ticketId,
          decision,
        }),
      )
    },
    // Prioritise the Ticket from the closed vocabulary.
    async prioritise(transport: ShellTransport, priority: TicketPriority): Promise<boolean> {
      return this.act(transport, (client, ticketId, version) =>
        client.commandTicketPrioritise({
          mutation: { optimistic_version: version, idempotency_key: crypto.randomUUID() },
          ticket_id: ticketId,
          priority,
        }),
      )
    },
    // Edit the title a Bug or Task carries or the slice description
    // an Implementation carries; exactly the field the kind owns is
    // sent.
    async edit(transport: ShellTransport, fields: TicketEditFields): Promise<boolean> {
      return this.act(transport, (client, ticketId, version) =>
        client.commandTicketEdit({
          mutation: { optimistic_version: version, idempotency_key: crypto.randomUUID() },
          ticket_id: ticketId,
          ...(fields.title !== undefined ? { title: fields.title } : {}),
          ...(fields.slice !== undefined ? { slice: fields.slice } : {}),
        }),
      )
    },
    // Recovery moves the Ticket past the rules through the one audited
    // override, carrying the operator and reason its audit row
    // records.
    async override(
      transport: ShellTransport,
      to: TicketState,
      who: string,
      why: string,
    ): Promise<boolean> {
      return this.act(transport, (client, ticketId, version) =>
        client.commandTicketEmergencyOverride({
          mutation: { optimistic_version: version, idempotency_key: crypto.randomUUID() },
          ticket_id: ticketId,
          to,
          who,
          why,
        }),
      )
    },
    // Run one lifecycle command against the open Ticket's current
    // version, replacing it with the record the command returns.
    async act(
      transport: ShellTransport,
      command: (client: KanbanClient, ticketId: number, version: number) => Promise<TicketRecord>,
    ): Promise<boolean> {
      const ticket = this.ticket
      if (ticket === null) {
        this.error = 'open a Ticket before acting on it'
        return false
      }
      try {
        this.ticket = await command(new KanbanClient(transport), ticket.id, ticket.version)
        this.error = null
      } catch (failure) {
        this.error = asApiError(failure).message
        return false
      }
      return true
    },
  },
})
