// The boot surface's connection state, driven entirely through the
// generated client: health queries set the phase, and the shell's
// connection announcements trigger a fresh query.
import { defineStore } from 'pinia'
import { KanbanClient } from '@kanban/contracts'
import type { ShellTransport } from '../core/transport'

// What the boot surface shows.
export type ConnectionPhase = 'connecting' | 'connected' | 'disconnected'

export const useConnectionStore = defineStore('connection', {
  state: () => ({
    phase: 'connecting' as ConnectionPhase,
    serviceVersion: null as string | null,
    lastEventSequence: null as number | null,
    booted: false,
  }),
  actions: {
    // Attach the store to a transport: keep the ordered event
    // stream's sequence visible, verify health, and re-verify on
    // every connection announcement. Safe to call more than once.
    async boot(transport: ShellTransport): Promise<void> {
      if (this.booted) {
        return
      }
      this.booted = true
      const client = new KanbanClient(transport)
      transport.subscribe((event) => {
        this.lastEventSequence = event.sequence
      })
      transport.onConnectionChange(() => {
        void this.verify(client)
      })
      await this.verify(client)
    },
    // The generated client is the source of truth: its health query
    // connects or it does not.
    async verify(client: KanbanClient): Promise<void> {
      this.phase = 'connecting'
      try {
        const health = await client.queryHealthGet()
        this.phase = 'connected'
        this.serviceVersion = health.service_version
      } catch {
        this.phase = 'disconnected'
      }
    },
  },
})
