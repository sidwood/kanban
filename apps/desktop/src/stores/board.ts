// The board state, driven entirely through the generated client: the
// Project's real Tickets arrive by `ticket.list`, the live cards'
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

// The states the readiness projection still speaks for: what holds a
// Ticket back matters only while the Ticket can still move, and the
// states that cannot — done, and the terminal cancelled and
// superseded — are history the board renders without a projection.
const FINISHED_STATES: readonly TicketState[] = ['done', 'cancelled', 'superseded']

/** Whether a Ticket's card can still be held back by what its
 * readiness projection counts. */
function canBeHeldBack(state: TicketState | undefined): boolean {
  return state !== undefined && !FINISHED_STATES.includes(state)
}

/** The cached projections that still speak for the Tickets the board
 * holds: an entry never outlives the Ticket it was asked for. */
function keptBlockers(
  tickets: readonly TicketRecord[],
  blockers: Record<number, TicketReadinessBlocker[]>,
): Record<number, TicketReadinessBlocker[]> {
  const kept: Record<number, TicketReadinessBlocker[]> = {}
  for (const ticket of tickets) {
    if (canBeHeldBack(ticket.state) && blockers[ticket.id] !== undefined) {
      kept[ticket.id] = blockers[ticket.id]
    }
  }
  return kept
}

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
    // projection decides which of them reach the columns, and the
    // blockers arrive beside the cards that can still be held back —
    // history never joins the query set, so it growing never grows
    // the traffic the refresh spends on the shared channel.
    async refresh(transport: ShellTransport, projectId: number): Promise<void> {
      try {
        const response = await new KanbanClient(transport).queryTicketList({
          project_id: projectId,
        })
        this.tickets = response.tickets
        const asking = response.tickets
          .filter((ticket) => canBeHeldBack(ticket.state))
          .map((ticket) => ticket.id)
        await this.refreshReadiness(transport, asking)
        this.loaded = true
        this.error = null
      } catch (failure) {
        this.error = asApiError(failure).message
      }
    },
    // Ask the core, once per Ticket still moving, what its readiness
    // projection holds back: dependencies first, then external
    // blockers. A Ticket that has finished is neither asked for nor
    // kept — what held it back can no longer matter.
    async refreshReadiness(
      transport: ShellTransport,
      ticketIds: readonly number[],
    ): Promise<void> {
      const client = new KanbanClient(transport)
      const asking = ticketIds.filter((ticketId) =>
        canBeHeldBack(this.tickets.find((ticket) => ticket.id === ticketId)?.state),
      )
      const responses = await Promise.all(
        asking.map((ticketId) => client.queryTicketReadiness({ ticket_id: ticketId })),
      )
      const blockers = { ...this.blockers }
      asking.forEach((ticketId, index) => {
        blockers[ticketId] = responses[index].blocked_by
      })
      this.blockers = keptBlockers(this.tickets, blockers)
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
