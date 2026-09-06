// The board state, driven entirely through the generated client: the
// Project's real Tickets arrive by `ticket.list`, and a drag becomes
// one `ticket.transition` carrying the Ticket's optimistic version and
// a fresh idempotency key. The core judges the move — a drag refused
// as agent-owned arrives here as that explanation, never swallowed
// (KAN-T24-AC3).
import { defineStore } from 'pinia'
import { KanbanClient } from '@kanban/contracts'
import type { TicketRecord, TicketState } from '@kanban/contracts'
import { asApiError } from '../core/transport'
import type { ShellTransport } from '../core/transport'

export const useBoardStore = defineStore('board', {
  state: () => ({
    tickets: [] as TicketRecord[],
    loaded: false,
    error: null as string | null,
  }),
  actions: {
    // Load the Project's Tickets, every state included; the board
    // projection decides which of them reach the columns.
    async refresh(transport: ShellTransport, projectId: number): Promise<void> {
      try {
        const response = await new KanbanClient(transport).queryTicketList({
          project_id: projectId,
        })
        this.tickets = response.tickets
        this.loaded = true
        this.error = null
      } catch (failure) {
        this.error = asApiError(failure).message
      }
    },
    // Send one drag to the core against the Ticket's current version,
    // replacing the held record with the one the command returns.
    async move(
      transport: ShellTransport,
      ticketId: number,
      to: TicketState,
    ): Promise<boolean> {
      const ticket = this.tickets.find((entry) => entry.id === ticketId)
      if (ticket === undefined) {
        this.error = `the board does not hold Ticket ${ticketId}`
        return false
      }
      try {
        const moved = await new KanbanClient(transport).commandTicketTransition({
          mutation: {
            optimistic_version: ticket.version,
            idempotency_key: crypto.randomUUID(),
          },
          ticket_id: ticketId,
          to,
        })
        this.tickets = this.tickets.map((entry) =>
          entry.id === moved.id ? moved : entry,
        )
        this.error = null
      } catch (failure) {
        this.error = asApiError(failure).message
        return false
      }
      return true
    },
  },
})
