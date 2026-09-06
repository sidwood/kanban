// The board state, driven entirely through the generated client: the
// Project's real Tickets arrive by `ticket.list`, the live cards'
// blockers arrive beside them as the core's own `ticket.readiness`
// projection, and a drag becomes one `ticket.transition` carrying the
// Ticket's optimistic version and a fresh idempotency key. The core
// judges the move — a drag refused as agent-owned arrives here as
// that explanation, never swallowed (KAN-T24-AC3). The board holds
// one Project at a time: leaving a Project takes its cards away
// before the next load settles, and a response that arrives for a
// Project the board has left is never rendered (KAN-T125).
import { defineStore } from 'pinia'
import { KanbanClient } from '@kanban/contracts'
import type {
  SpecRecord,
  TicketReadinessBlocker,
  TicketRecord,
  TicketState,
} from '@kanban/contracts'
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
    /** The Project whose Tickets the board holds; null when the board
     * holds none. */
    projectId: null as number | null,
    tickets: [] as TicketRecord[],
    /** The Project's Specs, loaded beside the Tickets that name them:
     * the canonical number a card renders comes from the record, never
     * from the row id a Ticket carries. */
    specs: [] as SpecRecord[],
    /** The core's readiness projection per Ticket id, loaded beside
     * the Tickets it speaks for. */
    blockers: {} as Record<number, TicketReadinessBlocker[]>,
    loaded: false,
    error: null as string | null,
    // The loads issued so far, so only the latest one — the Project
    // actually on display — ever writes state.
    issued: 0,
  }),
  getters: {
    /** What holds one Ticket back, as the core computes it. */
    blockersFor: (state) => (ticketId: number): readonly TicketReadinessBlocker[] =>
      state.blockers[ticketId] ?? [],
  },
  actions: {
    // Forget the board of the Project the operator has left: no card,
    // count, blocker, or Spec of one Project outlives the navigation it
    // belonged to, and any load still on the wire for it is
    // superseded and writes nothing.
    clear(): void {
      this.issued += 1
      this.projectId = null
      this.tickets = []
      this.specs = []
      this.blockers = {}
      this.loaded = false
      this.error = null
    },
    // Load the Project's Tickets, every state included, and its Specs
    // beside them; the board projection decides which of them reach
    // the columns, and the blockers arrive beside the cards that can
    // still be held back — history never joins the query set, so it
    // growing never grows the traffic the refresh spends on the shared
    // channel.
    async refresh(transport: ShellTransport, projectId: number): Promise<void> {
      // Entering a Project takes the previous one's board away before
      // any response settles, so nothing of one Project renders under
      // another's heading (KAN-T125-AC1).
      this.clear()
      const attempt = this.issued
      this.projectId = projectId
      try {
        const client = new KanbanClient(transport)
        const [tickets, specs] = await Promise.all([
          client.queryTicketList({ project_id: projectId }),
          client.querySpecList({ project_id: projectId }),
        ])
        // Only the load for the Project on display writes state: a
        // slower answer for a Project the board has left never
        // renders (KAN-T125-AC2).
        if (attempt !== this.issued) return
        this.tickets = tickets.tickets
        this.specs = specs.specs
        const asking = tickets.tickets
          .filter((ticket) => canBeHeldBack(ticket.state))
          .map((ticket) => ticket.id)
        await this.refreshReadiness(transport, asking)
        if (attempt !== this.issued) return
        this.loaded = true
        this.error = null
      } catch (failure) {
        // A failure for a Project the board has left belongs to that
        // Project, not to the one on display (KAN-T125-AC2).
        if (attempt !== this.issued) return
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
      // The board speaks for one Project: a card that reached it from
      // another Project never mutates through it, however it came to
      // be held (KAN-T125-AC3).
      if (ticket.project_id !== this.projectId) {
        this.error = `Ticket ${ticketId} does not belong to the board's Project`
        return false
      }
      // A move the board issued before it changed Project belongs to
      // the Project it was issued for: its result — landed or refused
      // — renders nowhere here.
      const attempt = this.issued
      try {
        const moved = await new KanbanClient(transport).commandTicketTransition({
          mutation: {
            optimistic_version: ticket.version,
            idempotency_key: crypto.randomUUID(),
          },
          ticket_id: ticketId,
          to,
        })
        if (attempt !== this.issued) return false
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
          if (attempt === this.issued) {
            this.error = asApiError(projection).message
          }
        }
      } catch (failure) {
        if (attempt !== this.issued) return false
        this.error = asApiError(failure).message
        return false
      }
      return true
    },
  },
})
