// The dependency editor state, driven entirely through the generated
// client: the Tickets of one Project to pick from, and — for the
// picked Ticket — its registered dependencies (which may cross Specs
// and registered Projects), its explicit external blockers, and the
// computed readiness the core projects from exactly those facts.
// Readiness is a query, never a mutation; every edit guards on the
// Ticket's aggregate version and reports a refusal, never swallowing
// it (KAN-S4-US5).
import { defineStore } from 'pinia'
import { KanbanClient } from '@kanban/contracts'
import type {
  TicketDependenciesResponse,
  TicketReadinessResponse,
  TicketRecord,
} from '@kanban/contracts'
import { asApiError } from '../core/transport'
import type { ShellTransport } from '../core/transport'

export const useDependencyEditorStore = defineStore('dependency-editor', {
  state: () => ({
    tickets: [] as TicketRecord[],
    sourceTickets: [] as TicketRecord[],
    dependencies: null as TicketDependenciesResponse | null,
    readiness: null as TicketReadinessResponse | null,
    loaded: false,
    error: null as string | null,
  }),
  actions: {
    // Load every Ticket of one Project, terminal lifecycle states
    // included, for the waiting-Ticket picker.
    async refresh(transport: ShellTransport, projectId: number): Promise<void> {
      try {
        const response = await new KanbanClient(transport).queryTicketList({ project_id: projectId })
        this.tickets = response.tickets
        this.loaded = true
        this.error = null
      } catch (failure) {
        this.error = asApiError(failure).message
      }
    },
    // Load the blocking-Ticket picker's candidates from any Project:
    // dependencies may cross Projects.
    async loadSource(transport: ShellTransport, projectId: number): Promise<void> {
      try {
        const response = await new KanbanClient(transport).queryTicketList({ project_id: projectId })
        this.sourceTickets = response.tickets
        this.error = null
      } catch (failure) {
        this.error = asApiError(failure).message
      }
    },
    // Open one Ticket: its dependencies and blockers beside the
    // readiness the core computes from them.
    async open(transport: ShellTransport, ticketId: number): Promise<void> {
      const client = new KanbanClient(transport)
      try {
        const [dependencies, readiness] = await Promise.all([
          client.queryTicketDependencies({ ticket_id: ticketId }),
          client.queryTicketReadiness({ ticket_id: ticketId }),
        ])
        this.dependencies = dependencies
        this.readiness = readiness
        this.error = null
      } catch (failure) {
        this.error = asApiError(failure).message
      }
    },
    // Register that `dependsOn` must land before `ticket` may begin.
    // Reports whether the edge landed; a refusal is reported and the
    // open Ticket stands.
    async addDependency(
      transport: ShellTransport,
      ticketId: number,
      dependsOn: number,
    ): Promise<boolean> {
      return this.mutate(transport, ticketId, (client, version) =>
        client.commandTicketDependencyAdd({
          mutation: { optimistic_version: version, idempotency_key: crypto.randomUUID() },
          from_ticket: dependsOn,
          to_ticket: ticketId,
        }),
      )
    },
    // Remove one registered dependency.
    async removeDependency(
      transport: ShellTransport,
      ticketId: number,
      fromTicket: number,
    ): Promise<boolean> {
      return this.mutate(transport, ticketId, (client, version) =>
        client.commandTicketDependencyRemove({
          mutation: { optimistic_version: version, idempotency_key: crypto.randomUUID() },
          from_ticket: fromTicket,
          to_ticket: ticketId,
        }),
      )
    },
    // Record one explicit external blocker naming the unregistered
    // work the Ticket waits on.
    async addBlocker(
      transport: ShellTransport,
      ticketId: number,
      description: string,
    ): Promise<boolean> {
      return this.mutate(transport, ticketId, (client, version) =>
        client.commandTicketBlockerAdd({
          mutation: { optimistic_version: version, idempotency_key: crypto.randomUUID() },
          ticket_id: ticketId,
          description,
        }),
      )
    },
    // Remove one recorded blocker; removal is the operator action
    // that clears it.
    async removeBlocker(
      transport: ShellTransport,
      ticketId: number,
      blockerId: number,
    ): Promise<boolean> {
      return this.mutate(transport, ticketId, (client, version) =>
        client.commandTicketBlockerRemove({
          mutation: { optimistic_version: version, idempotency_key: crypto.randomUUID() },
          ticket_id: ticketId,
          blocker_id: blockerId,
        }),
      )
    },
    // Run one dependency command against the open Ticket's current
    // version, then reopen the Ticket so the editor always shows the
    // stored truth.
    async mutate(
      transport: ShellTransport,
      ticketId: number,
      command: (client: KanbanClient, version: number) => Promise<TicketDependenciesResponse>,
    ): Promise<boolean> {
      const version = this.dependencies?.version ?? 0
      try {
        this.dependencies = await command(new KanbanClient(transport), version)
        this.error = null
      } catch (failure) {
        this.error = asApiError(failure).message
        return false
      }
      await this.open(transport, ticketId)
      return true
    },
  },
})
