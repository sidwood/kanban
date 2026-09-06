// The board state, driven entirely through the generated client: the
// Project's real Tickets arrive by `ticket.list`, each Ticket's
// blockers arrive beside them as the core's own `ticket.readiness`
// projection, and a drag becomes one `ticket.transition` carrying the
// Ticket's optimistic version and a fresh idempotency key. The core
// judges the move — a drag refused as agent-owned arrives here as
// that explanation, never swallowed (KAN-T24-AC3).
import { defineStore } from 'pinia'
import { KanbanClient } from '@kanban/contracts'
import type { TicketReadinessBlocker, TicketRecord, TicketState } from '@kanban/contracts'
import { asApiError } from '../core/transport'
import type { ShellTransport } from '../core/transport'

export const useBoardStore = defineStore('board', {
  state: () => ({
    tickets: [] as TicketRecord[],
    /** The core's readiness projection per Ticket id, loaded beside
     * the Tickets it speaks for. */
    blockers: {} as Record<number, TicketReadinessBlocker[]>,
    loaded: false,
    error: null as string | null,
  }),
  getters: {
    /** What holds one Ticket back, as the core computes it. */
    blockersFor: (state) => (ticketId: number): readonly TicketReadinessBlocker[] =>
      state.blockers[ticketId] ?? [],
  },
  actions: {
    // Load the Project's Tickets, every state included; the board
    // projection decides which of them reach the columns, and each
    // Ticket's blockers arrive with them.
    async refresh(transport: ShellTransport, projectId: number): Promise<void> {
      try {
        const response = await new KanbanClient(transport).queryTicketList({
          project_id: projectId,
        })
        this.tickets = response.tickets
        await this.refreshReadiness(transport, response.tickets.map((ticket) => ticket.id))
        this.loaded = true
        this.error = null
      } catch (failure) {
        this.error = asApiError(failure).message
      }
    },
    // Ask the core, once per Ticket, what its readiness projection
    // holds back: dependencies first, then external blockers.
    async refreshReadiness(
      transport: ShellTransport,
      ticketIds: readonly number[],
    ): Promise<void> {
      const client = new KanbanClient(transport)
      const responses = await Promise.all(
        ticketIds.map((ticketId) => client.queryTicketReadiness({ ticket_id: ticketId })),
      )
      const blockers = { ...this.blockers }
      ticketIds.forEach((ticketId, index) => {
        blockers[ticketId] = responses[index].blocked_by
      })
      this.blockers = blockers
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
        // The move may have changed what holds the Ticket back; the
        // projection is the core's to recompute, and a failure to
        // refresh it never rolls the move back.
        try {
          await this.refreshReadiness(transport, [moved.id])
        } catch (projection) {
          this.error = asApiError(projection).message
        }
      } catch (failure) {
        this.error = asApiError(failure).message
        return false
      }
      return true
    },
  },
})
