// The board state, driven entirely through the generated client: one
// `board.global` query carries the scope's filter in and the
// projection back — cards already grouped and in the deterministic
// order, the values every filter axis offers beside them. The core
// owns the filtering, the group mapping, and the order; this store
// holds the projection for the scope on display, the readiness the
// core computes for each card that can still be held back, the run
// records whose effective profiles the cards wear, and the reviewer
// slots configured on each Implementation. A drag becomes one
// `ticket.transition` carrying the Ticket's optimistic version and a
// fresh idempotency key; the core judges the move, and a refusal — an
// agent-owned drag above all — arrives here as that explanation,
// never swallowed (KAN-T24-AC3). The board holds one scope at a time:
// leaving a scope takes its cards away before the next load settles,
// and a response that arrives for a scope the board has left is never
// rendered (KAN-T125).
import { defineStore } from 'pinia'
import { KanbanClient } from '@kanban/contracts'
import type {
  BoardFilter,
  BoardFilterOptions,
  BoardGlobalCard,
  CriterionBindingRecord,
  RunRecord,
  TicketReadinessBlocker,
  TicketRecord,
  TicketReviewConfigRecord,
  TicketState,
} from '@kanban/contracts'
import { asApiError } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import { activeFilterCount, scopedFilter } from '../views/board-filters'
import { boardGroupForState } from '../views/board-layout'
import { emptyFilter, wireFilter } from '../views/global-board-filters'
import type { BoardScope } from './scope'

/** The execution facts a card's profile chips wear. */
export interface ExecutionFacts {
  effective: string
  fallback: boolean
}

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

/** The cached projections that still speak for the cards the board
 * holds: an entry never outlives the card it was asked for. */
function keptBlockers(
  cards: readonly BoardGlobalCard[],
  blockers: Record<number, TicketReadinessBlocker[]>,
): Record<number, TicketReadinessBlocker[]> {
  const kept: Record<number, TicketReadinessBlocker[]> = {}
  for (const card of cards) {
    const ticket = card.ticket
    if (canBeHeldBack(ticket.state) && blockers[ticket.id] !== undefined) {
      kept[ticket.id] = blockers[ticket.id]
    }
  }
  return kept
}

/** The reviewer names one configuration carries, stage by stage and
 * slot by slot: a profile by its name, a human slot as `Human`. */
export function reviewerNames(config: TicketReviewConfigRecord | null | undefined): string[] {
  if (!config) return []
  return config.stages.flatMap((stage) =>
    stage.slots.map((slot) => (slot.occupant.kind === 'human' ? 'Human' : slot.occupant.name)),
  )
}

export const useBoardStore = defineStore('board', {
  state: () => ({
    /** The scope whose projection the board holds; null when it
     * holds none. */
    scope: null as BoardScope | null,
    /** The filtered projection, in the core's deterministic order. */
    cards: [] as BoardGlobalCard[],
    /** The values every reference axis offers, as the core read them. */
    options: null as BoardFilterOptions | null,
    /** The core's readiness projection per Ticket id, loaded beside
     * the cards it speaks for. */
    blockers: {} as Record<number, TicketReadinessBlocker[]>,
    /** The reviewer names configured per Implementation, in slot order. */
    reviewers: {} as Record<number, string[]>,
    /** The criterion evidence bindings the core holds per Ticket:
     * what the cards' progress is counted from. */
    bindings: {} as Record<number, CriterionBindingRecord[]>,
    /** The states a human drag may move each Ticket to now, as the
     * core judges them. */
    transitions: {} as Record<number, TicketState[]>,
    /** The run records of every Project on the board, by Project. */
    runs: {} as Record<number, RunRecord[]>,
    /** How many cards the scope holds with no filter beyond the scope
     * itself, read beside every filtered load; null until a load has
     * landed. */
    scopeTotal: null as number | null,
    loaded: false,
    /** A re-query of the scope on the wire while its cards stay up. */
    loading: false,
    error: null as string | null,
    // The loads issued so far, so only the latest one — the scope
    // actually on display — ever writes state.
    issued: 0,
  }),
  getters: {
    /** What holds one Ticket back, as the core computes it. */
    blockersFor: (state) => (ticketId: number): readonly TicketReadinessBlocker[] =>
      state.blockers[ticketId] ?? [],
    /** The reviewers configured on one Ticket, in slot order. */
    reviewersFor: (state) => (ticketId: number): readonly string[] =>
      state.reviewers[ticketId] ?? [],
    /** The criterion bindings the core holds for one Ticket. */
    bindingsFor: (state) => (ticketId: number): readonly CriterionBindingRecord[] =>
      state.bindings[ticketId] ?? [],
    /** Where a human drag may move one Ticket now: the core's own
     * answer, and nothing a client worked out for itself. A Ticket
     * the board has not read moves nowhere. */
    legalTargetsFor: (state) => (ticketId: number): readonly TicketState[] =>
      state.transitions[ticketId] ?? [],
    /** One card by the Ticket it carries, when the board holds it. */
    cardOf: (state) => (ticketId: number): BoardGlobalCard | undefined =>
      state.cards.find((card) => card.ticket.id === ticketId),
    /** The execution facts of the run executing `ticketId` now: the
     * effective profile it froze and whether it fell back from the
     * planned one. A Ticket with no executing run has none. */
    executionFor: (state) => (ticketId: number): ExecutionFacts | null => {
      const executing = Object.values(state.runs)
        .flat()
        .filter((run) => run.status === 'executing' && run.ticket_id === ticketId)
      const newest = executing[executing.length - 1]
      return newest ? { effective: newest.effective.name, fallback: newest.fallback } : null
    },
    /** Every run the Ticket has ever minted, oldest first. */
    attemptsFor: (state) => (ticketId: number): readonly RunRecord[] =>
      Object.values(state.runs)
        .flat()
        .filter((run) => run.ticket_id === ticketId)
        .sort((left, right) => left.created_at - right.created_at),
    /** The Projects the cards on show belong to, in first-seen order. */
    projectsPresent: (state): readonly number[] => {
      const seen: number[] = []
      for (const card of state.cards) {
        if (!seen.includes(card.ticket.project_id)) seen.push(card.ticket.project_id)
      }
      return seen
    },
  },
  actions: {
    // Forget the board of the scope the operator has left: no card,
    // count, blocker, or run of one scope outlives the navigation it
    // belonged to, and any load still on the wire for it is
    // superseded and writes nothing.
    clear(): void {
      this.issued += 1
      this.scope = null
      this.cards = []
      this.options = null
      this.blockers = {}
      this.reviewers = {}
      this.bindings = {}
      this.transitions = {}
      this.runs = {}
      this.scopeTotal = null
      this.loaded = false
      this.loading = false
      this.error = null
    },
    // Load the projection the scope's filter selects — and, beside a
    // filtered one, the whole scope, so the toolbar can say what the
    // filter hides — then the readiness beside the cards that can
    // still be held back, the reviewers of every Implementation, and
    // the runs of every Project on the board. Entering another scope
    // takes the previous one's board away before any response
    // settles (KAN-T125-AC1); re-querying the same scope keeps its
    // cards up until the new projection lands.
    async refresh(transport: ShellTransport, scope: BoardScope, filter: BoardFilter): Promise<void> {
      if (this.scope !== scope) {
        this.clear()
        this.scope = scope
      }
      this.issued += 1
      const attempt = this.issued
      this.loading = true
      try {
        const client = new KanbanClient(transport)
        const filtered = activeFilterCount(filter, scope) > 0
        const [whole, board] = await Promise.all([
          filtered
            ? client.queryBoardGlobal({ filter: wireFilter(scopedFilter(emptyFilter(), scope)) })
            : Promise.resolve(null),
          client.queryBoardGlobal({ filter: wireFilter(scopedFilter(filter, scope)) }),
        ])
        // Only the load for the scope on display writes state: a
        // slower answer for a scope the board has left never renders
        // (KAN-T125-AC2).
        if (attempt !== this.issued) return
        this.cards = board.cards
        this.options = board.options
        this.scopeTotal = whole ? whole.cards.length : board.cards.length
        this.loaded = true
        this.loading = false
        this.error = null
      } catch (failure) {
        // A failure for a scope the board has left belongs to that
        // scope, not to the one on display (KAN-T125-AC2).
        if (attempt !== this.issued) return
        this.loading = false
        this.error = asApiError(failure).message
        return
      }
      try {
        const asking = this.cards
          .filter((card) => canBeHeldBack(card.ticket.state))
          .map((card) => card.ticket.id)
        const reviewed = this.cards
          .filter((card) => card.ticket.kind === 'implementation')
          .map((card) => card.ticket.id)
        // Only a Task answers a human drag, so only a Task's legal
        // moves are worth reading: the core answers every other kind
        // with nothing at all (DR-LC-07, DR-LC-08).
        const draggable = this.cards
          .filter((card) => card.ticket.kind === 'task')
          .map((card) => card.ticket.id)
        await Promise.all([
          this.refreshReadiness(transport, asking, attempt),
          this.refreshReviewers(transport, reviewed, attempt),
          this.refreshBindings(transport, this.cards.map((card) => card.ticket.id), attempt),
          this.refreshTransitions(transport, draggable, attempt),
          this.loadRuns(transport, this.projectsPresent, attempt),
        ])
      } catch (failure) {
        // The projection stands; what could not be read beside it is
        // reported rather than invented.
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
      issuedFor?: number,
    ): Promise<void> {
      const attempt = issuedFor ?? this.issued
      const client = new KanbanClient(transport)
      const asking = ticketIds.filter((ticketId) =>
        canBeHeldBack(this.cardOf(ticketId)?.ticket.state),
      )
      const responses = await Promise.all(
        asking.map((ticketId) => client.queryTicketReadiness({ ticket_id: ticketId })),
      )
      if (attempt !== this.issued) return
      const blockers = { ...this.blockers }
      asking.forEach((ticketId, index) => {
        blockers[ticketId] = responses[index].blocked_by
      })
      this.blockers = keptBlockers(this.cards, blockers)
    },
    // The reviewer slots configured on each Implementation, read from
    // the same record the review configuration edits; none
    // configured leaves the region off the card.
    async refreshReviewers(
      transport: ShellTransport,
      ticketIds: readonly number[],
      issuedFor?: number,
    ): Promise<void> {
      const attempt = issuedFor ?? this.issued
      const client = new KanbanClient(transport)
      const responses = await Promise.all(
        ticketIds.map((ticketId) => client.queryTicketReviewConfig({ ticket_id: ticketId })),
      )
      if (attempt !== this.issued) return
      const reviewers: Record<number, string[]> = {}
      ticketIds.forEach((ticketId, index) => {
        reviewers[ticketId] = reviewerNames(responses[index]?.config)
      })
      this.reviewers = reviewers
    },
    // What the core says each Ticket's criteria have reached: the
    // bindings its progress is counted from, read once per card
    // beside the projection they belong to (DR-BP-08).
    async refreshBindings(
      transport: ShellTransport,
      ticketIds: readonly number[],
      issuedFor?: number,
    ): Promise<void> {
      const attempt = issuedFor ?? this.issued
      const client = new KanbanClient(transport)
      const responses = await Promise.all(
        ticketIds.map((ticketId) => client.queryCriterionBindings({ ticket_id: ticketId })),
      )
      if (attempt !== this.issued) return
      const bindings: Record<number, CriterionBindingRecord[]> = {}
      ticketIds.forEach((ticketId, index) => {
        bindings[ticketId] = responses[index]?.bindings ?? []
      })
      this.bindings = bindings
    },
    // Where the core says each draggable Ticket may go: the whole
    // legality judgement — the canonical lifecycle, the kind's
    // ownership, and the Ticket's own gates — stays in the core, and
    // a surface offering moves offers exactly these (KAN-T137-AC5).
    async refreshTransitions(
      transport: ShellTransport,
      ticketIds: readonly number[],
      issuedFor?: number,
    ): Promise<void> {
      const attempt = issuedFor ?? this.issued
      const client = new KanbanClient(transport)
      const responses = await Promise.all(
        ticketIds.map((ticketId) => client.queryTicketTransitions({ ticket_id: ticketId })),
      )
      if (attempt !== this.issued) return
      const transitions: Record<number, TicketState[]> = {}
      ticketIds.forEach((ticketId, index) => {
        transitions[ticketId] = responses[index]?.targets ?? []
      })
      this.transitions = transitions
    },
    // The runs of every Project on the board, one read per Project.
    async loadRuns(
      transport: ShellTransport,
      projectIds: readonly number[],
      issuedFor?: number,
    ): Promise<void> {
      const attempt = issuedFor ?? this.issued
      const client = new KanbanClient(transport)
      const responses = await Promise.all(
        projectIds.map((projectId) => client.queryRunList({ project_id: projectId })),
      )
      if (attempt !== this.issued) return
      const runs: Record<number, RunRecord[]> = {}
      responses.forEach((response) => {
        runs[response.project_id] = response.runs
      })
      this.runs = runs
    },
    // Read one Project's runs again — after a recovery, say — and
    // keep every other Project's beside them.
    async refreshRuns(transport: ShellTransport, projectId: number): Promise<void> {
      const attempt = this.issued
      const response = await new KanbanClient(transport).queryRunList({ project_id: projectId })
      if (attempt !== this.issued) return
      this.runs = { ...this.runs, [response.project_id]: response.runs }
    },
    // Send one drag to the core against the Ticket's current version,
    // replacing the held record with the one the command returns; the
    // card follows its new state into the group the mapping fixes.
    async move(transport: ShellTransport, ticketId: number, to: TicketState): Promise<boolean> {
      const card = this.cardOf(ticketId)
      // The board speaks for one scope: a card it does not hold never
      // mutates through it, however it came to be dragged
      // (KAN-T125-AC3).
      if (card === undefined) {
        this.error = `the board does not hold Ticket ${ticketId}`
        return false
      }
      // A move the board issued before it changed scope belongs to
      // the scope it was issued for: its result — landed or refused
      // — renders nowhere here.
      const attempt = this.issued
      try {
        const moved: TicketRecord | undefined = await new KanbanClient(transport).commandTicketTransition({
          mutation: {
            optimistic_version: card.ticket.version,
            idempotency_key: crypto.randomUUID(),
          },
          ticket_id: ticketId,
          to,
        })
        if (attempt !== this.issued) return false
        if (moved === undefined || moved === null) {
          this.error = `the core returned no record for the move of Ticket ${ticketId}`
          return false
        }
        const group = boardGroupForState(moved.state)
        this.cards = this.cards.flatMap((entry) => {
          if (entry.ticket.id !== moved.id) return [entry]
          // A record that left the active board leaves the cards.
          return group === undefined ? [] : [{ ...entry, ticket: moved, group }]
        })
        this.error = null
        // The move may have changed what holds the Ticket back; the
        // projection is the core's to recompute, and a failure to
        // refresh it never rolls the move back.
        try {
          await this.refreshReadiness(transport, [moved.id], attempt)
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
