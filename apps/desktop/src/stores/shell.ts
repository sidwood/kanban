// The shell's own session state: whether the window is narrow, which
// forces the icon rail whatever the operator prefers, and the count
// the Attention inbox badge wears, read from the same
// `attention.list` the inbox itself reads and re-read on the same
// cadence, since no live event announces an Attention item. Whether
// the rail stands open is not session state — it is the operator's
// arrangement, and the core holds it (see `preferences.ts`).
import { defineStore } from 'pinia'
import { KanbanClient } from '@kanban/contracts'
import type { ShellTransport } from '../core/transport'
import { usePreferencesStore } from './preferences'

/** Below this width the rail keeps only its icons and the board
 * stacks its groups. */
export const NARROW_WIDTH_PX = 900

/** How often the badge re-reads the inbox, the inbox's own cadence. */
export const ATTENTION_POLL_MS = 5000

export const useShellStore = defineStore('shell', {
  state: () => ({
    /** Whether the window is narrower than the rail and board need. */
    narrow: false,
    /** Attention items still waiting on the operator; null until
     * read, and null again when the inbox cannot be read. */
    attentionCount: null as number | null,
  }),
  getters: {
    /** Whether the rail shows its labels: the arrangement the core
     * holds, unless the window is too narrow for them. */
    railExpanded: (state): boolean => usePreferencesStore().railOpen && !state.narrow,
  },
  actions: {
    setNarrow(narrow: boolean): void {
      this.narrow = narrow
    },
    // The badge counts what the inbox would list by default: active
    // items nobody has acknowledged.
    async refreshAttention(transport: ShellTransport): Promise<void> {
      try {
        const response = await new KanbanClient(transport).queryAttentionList({})
        this.attentionCount = response.items.filter(
          (item) => item.active && item.acknowledged_by == null,
        ).length
      } catch {
        this.attentionCount = null
      }
    },
  },
})
