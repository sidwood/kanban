// Run state per Project: the run records the core owns, loaded beside
// the Tickets whose cards wear them (KAN-S9-US3). The effective
// profile a card shows during execution comes from the run's frozen
// snapshot, never from the catalogue the assignment still names.
import { defineStore } from 'pinia'
import { KanbanClient } from '@kanban/contracts'
import type { RunRecord } from '@kanban/contracts'
import { asApiError } from '../core/transport'
import type { ShellTransport } from '../core/transport'

/** The execution facts a card's profile chips wear. */
export interface ExecutionFacts {
  effective: string
  fallback: boolean
}

export const useRunsStore = defineStore('runs', {
  state: () => ({
    projectId: null as number | null,
    runs: [] as RunRecord[],
    loaded: false,
    error: null as string | null,
  }),
  getters: {
    /** The execution facts of the run executing `ticketId` now: the
     * effective profile it froze and whether it fell back from the
     * planned one. A Ticket with no executing run — before dispatch —
     * has none. */
    executionFor:
      (state) =>
      (ticketId: number): ExecutionFacts | null => {
        const executing = state.runs.filter(
          (run) => run.status === 'executing' && run.ticket_id === ticketId,
        )
        const newest = executing[executing.length - 1]
        return newest
          ? { effective: newest.effective.name, fallback: newest.fallback }
          : null
      },
  },
  actions: {
    async load(transport: ShellTransport, projectId: number): Promise<void> {
      this.projectId = projectId
      try {
        const response = await new KanbanClient(transport).queryRunList({ project_id: projectId })
        this.runs = response.runs
        this.loaded = true
        this.error = null
      } catch (failure) {
        this.error = asApiError(failure).message
      }
    },
  },
})
