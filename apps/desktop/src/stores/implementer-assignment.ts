// The ordinary implementer assignment (KAN-T140-AC5, DR-EP-03).
// Assignment is an act of its own — nothing about a Ticket's review
// has to exist first — so this store speaks only to `ticket.list` and
// `ticket.assign`. Every read and every command is bound to the
// Project scope it was issued in: a Project the operator has left can
// neither answer into the one they are in nor lend it a Ticket to
// assign.
import { defineStore } from 'pinia'
import { KanbanClient } from '@kanban/contracts'
import type { TicketRecord } from '@kanban/contracts'
import { asApiError } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import {
  adoptScope,
  emptyScope,
  issueCommand,
  projectScopeKey,
  scopeHolds,
} from '../core/scope-authority'

export const useImplementerAssignmentStore = defineStore('implementer-assignment', {
  state: () => ({
    ...emptyScope(),
    projectId: null as number | null,
    tickets: [] as TicketRecord[],
    loaded: false,
    error: null as string | null,
  }),
  actions: {
    // The Tickets of one Project, so an assignment names a real one.
    // The listing of the Project being left goes at once: a stale
    // Ticket must never be offered under a new Project's label.
    async load(transport: ShellTransport, projectId: number): Promise<void> {
      const claim = adoptScope(this, projectScopeKey(projectId))
      this.projectId = projectId
      this.tickets = []
      this.loaded = false
      this.error = null
      try {
        const response = await new KanbanClient(transport).queryTicketList({
          project_id: projectId,
        })
        if (!scopeHolds(this, claim)) return
        this.tickets = response.tickets
        this.loaded = true
        this.error = null
      } catch (failure) {
        if (!scopeHolds(this, claim)) return
        this.tickets = []
        this.loaded = false
        this.error = asApiError(failure).message
      }
    },
    // Name one catalogue entry as a Ticket's implementer. The Ticket
    // must belong to the Project on display, and an answer that
    // arrives after the scope moved on is dropped rather than applied
    // to a Project that never asked for it. Reports whether it
    // landed; a refusal is reported and the Ticket keeps the
    // assignment it had.
    async assign(
      transport: ShellTransport,
      ticketId: number,
      profile: string,
    ): Promise<boolean> {
      const scope = this.projectId
      const held = this.tickets.find((entry) => entry.id === ticketId)
      if (!held || scope === null || held.project_id !== scope) {
        this.error = `Ticket ${ticketId} is not in the Project on display`
        return false
      }
      const claim = issueCommand(this)
      try {
        const assigned = await new KanbanClient(transport).commandTicketAssign({
          mutation: { optimistic_version: held.version, idempotency_key: crypto.randomUUID() },
          ticket_id: ticketId,
          profile,
        })
        if (!scopeHolds(this, claim)) return false
        this.tickets = this.tickets.map((entry) => (entry.id === assigned.id ? assigned : entry))
        this.error = null
      } catch (failure) {
        if (!scopeHolds(this, claim)) return false
        this.error = asApiError(failure).message
        return false
      }
      return true
    },
  },
})
