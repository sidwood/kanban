// The staged review configuration state, driven entirely through the
// generated client: the ordered stages of parallel slots — each
// required or optional, occupied by a human or a named profile —
// that one Ticket's review carries, with the configure command that
// replaces the whole set and the read-back query that serves it
// (KAN-S10-US1, DR-EP-09). Separation refusals are reported, never
// swallowed, and the standing configuration survives them.
import { defineStore } from 'pinia'
import { KanbanClient } from '@kanban/contracts'
import type { TicketReviewConfigRecord, TicketReviewStage } from '@kanban/contracts'
import { asApiError } from '../core/transport'
import type { ShellTransport } from '../core/transport'

function mutationFor(optimisticVersion: number) {
  return { optimistic_version: optimisticVersion, idempotency_key: crypto.randomUUID() }
}

export const useReviewConfigStore = defineStore('review-config', {
  state: () => ({
    config: null as TicketReviewConfigRecord | null,
    loaded: false,
    error: null as string | null,
  }),
  actions: {
    // Load the picked Ticket's stored configuration; nothing stands
    // until one does.
    async refresh(transport: ShellTransport, ticketId: number): Promise<void> {
      try {
        const response = await new KanbanClient(transport).queryTicketReviewConfig({
          ticket_id: ticketId,
        })
        this.config = response.config ?? null
        this.loaded = true
        this.error = null
      } catch (failure) {
        this.error = asApiError(failure).message
      }
    },
    // Replace the Ticket's whole staged review. The optimistic
    // version is the standing configuration's, or zero while none
    // stands. Reports whether it landed; a refusal is reported and
    // the standing configuration stands.
    async configure(
      transport: ShellTransport,
      ticketId: number,
      stages: TicketReviewStage[],
    ): Promise<boolean> {
      const standing = this.config?.version ?? 0
      try {
        const record = await new KanbanClient(transport).commandTicketReviewConfigure({
          mutation: mutationFor(standing),
          ticket_id: ticketId,
          stages,
        })
        this.config = record
        this.error = null
      } catch (failure) {
        this.error = asApiError(failure).message
        return false
      }
      return true
    },
  },
})
