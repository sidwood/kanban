// The command palette state: navigation entries and global search
// hits composed into one read-only surface. Selecting an item only
// navigates; no command ever leaves the palette.
import { defineStore } from 'pinia'
import type { PaletteItem } from './palette-navigation'
import { filterNavigation, mergePaletteItems } from './palette-navigation'
import { useSearchStore } from './search'
import type { ShellTransport } from '../core/transport'

export const usePaletteStore = defineStore('palette', {
  state: () => ({
    open: false,
    selection: 0,
  }),
  getters: {
    items(): PaletteItem[] {
      const search = useSearchStore()
      return mergePaletteItems(filterNavigation(search.query), search.hits)
    },
  },
  actions: {
    openPalette(): void {
      this.open = true
      this.selection = 0
    },
    closePalette(): void {
      this.open = false
      this.selection = 0
      useSearchStore().clear()
    },
    moveSelection(delta: number): void {
      const count = this.items.length
      if (count === 0) {
        this.selection = 0
        return
      }
      this.selection = (this.selection + delta + count) % count
    },
    async setQuery(transport: ShellTransport, value: string): Promise<void> {
      const search = useSearchStore()
      search.setQuery(value)
      this.selection = 0
      await search.refresh(transport)
    },
    selectedItem(): PaletteItem | null {
      return this.items[this.selection] ?? null
    },
  },
})
