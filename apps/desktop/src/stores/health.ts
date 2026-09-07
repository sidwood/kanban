// Component health, driven entirely through the generated client:
// the one `health.get` answer reporting service, database,
// scheduler, MCP, Herdr, and Workspace state (KAN-S13-US5).
import { defineStore } from 'pinia'
import { KanbanClient } from '@kanban/contracts'
import type { HealthResponse } from '@kanban/contracts'
import { asApiError } from '../core/transport'
import type { ShellTransport } from '../core/transport'

export const useHealthStore = defineStore('health', {
  state: () => ({
    health: null as HealthResponse | null,
    error: null as string | null,
  }),
  actions: {
    // The generated client is the one path: the query is the probe.
    async refresh(transport: ShellTransport): Promise<void> {
      try {
        this.health = await new KanbanClient(transport).queryHealthGet({})
        this.error = null
      } catch (failure) {
        this.error = asApiError(failure).message
      }
    },
  },
})
