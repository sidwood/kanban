// Global search state, driven entirely through the generated client:
// one `search.global` query carries the operator's text in and the
// hits back — already in the core's deterministic order. The store
// never issues a command.
import { defineStore } from 'pinia'
import { KanbanClient } from '@kanban/contracts'
import type { SearchGlobalHit } from '@kanban/contracts'
import { asApiError } from '../core/transport'
import type { ShellTransport } from '../core/transport'

export const useSearchStore = defineStore('search', {
  state: () => ({
    query: '',
    hits: [] as SearchGlobalHit[],
    loading: false,
    error: null as string | null,
    issued: 0,
  }),
  actions: {
    setQuery(value: string): void {
      this.query = value
    },
    clear(): void {
      this.query = ''
      this.hits = []
      this.loading = false
      this.error = null
      this.issued += 1
    },
    async refresh(transport: ShellTransport): Promise<void> {
      const issued = ++this.issued
      const query = this.query.trim()
      if (!query) {
        this.hits = []
        this.loading = false
        this.error = null
        return
      }
      this.loading = true
      this.error = null
      try {
        const response = await new KanbanClient(transport).querySearchGlobal({ q: query })
        if (issued !== this.issued) return
        this.hits = response.hits
        this.loading = false
      } catch (error) {
        if (issued !== this.issued) return
        this.error = asApiError(error).message
        this.hits = []
        this.loading = false
      }
    },
  },
})
