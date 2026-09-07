// The global board state, driven entirely through the generated
// client: one `board.global` query carries the filter in and the
// projection back — cards already grouped and ordered, options for
// every axis beside them. The core owns the filtering, the group
// mapping, and the order; this store only holds the filter the
// operator is building and the projection that came back for it. A
// response that settles after the board was cleared renders nothing.
import { defineStore } from 'pinia'
import { KanbanClient } from '@kanban/contracts'
import type {
  BoardFilter,
  BoardFilterOptions,
  BoardGlobalCard,
} from '@kanban/contracts'
import { asApiError } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import { emptyFilter, toggleValue, wireFilter } from '../views/global-board-filters'

/// The axes whose values are numeric identities.
export type BoardIdAxis = 'initiatives' | 'projects' | 'plans' | 'specs' | 'lanes'
/// The axes whose values are words of a closed vocabulary, or a
/// profile's name.
export type BoardWordAxis = 'kinds' | 'states' | 'priorities' | 'attention' | 'profiles'

export const useGlobalBoardStore = defineStore('global-board', {
  state: () => ({
    /** The filter the next query carries; empty is the whole board. */
    filter: {
      initiatives: [],
      projects: [],
      plans: [],
      specs: [],
      kinds: [],
      states: [],
      priorities: [],
      lanes: [],
      profiles: [],
      attention: [],
    } as BoardFilter,
    /** The filtered projection, in the core's deterministic order. */
    cards: [] as BoardGlobalCard[],
    /** The values every reference axis offers, as the core read them. */
    options: null as BoardFilterOptions | null,
    loaded: false,
    error: null as string | null,
    // The loads issued so far, so only the latest one ever writes
    // state.
    issued: 0,
  }),
  actions: {
    // Toggle one identity axis: Initiatives, Projects, Plans, Specs,
    // and Lanes.
    toggleId(axis: BoardIdAxis, id: number): void {
      this.filter = {
        ...this.filter,
        [axis]: toggleValue(this.filter[axis] ?? [], id),
      }
    },
    // Toggle one word axis: kind, state, priority, attention class,
    // or a profile's name. Every word axis serialises its values as
    // strings, so one string set serves them all.
    toggleWord(axis: BoardWordAxis, value: string): void {
      // The spread widens a computed key; the axis and value are the
      // ones this action was handed.
      this.filter = {
        ...this.filter,
        [axis]: toggleValue((this.filter[axis] ?? []) as string[], value),
      } as BoardFilter
    },
    // Take every axis back to empty: the whole board again.
    resetFilter(): void {
      this.filter = {
        initiatives: [],
        projects: [],
        plans: [],
        specs: [],
        kinds: [],
        states: [],
        priorities: [],
        lanes: [],
        profiles: [],
        attention: [],
      }
    },
    // Adopt one filter whole, every axis exactly as the view that
    // owns it recorded them — the restoration a saved view performs
    // on switch (DR-BP-05), never a merge.
    setFilter(filter: BoardFilter): void {
      this.filter = { ...emptyFilter(), ...filter }
    },
    // Forget the board: a load still on the wire for it is
    // superseded and writes nothing.
    clear(): void {
      this.issued += 1
      this.resetFilter()
      this.cards = []
      this.options = null
      this.loaded = false
      this.error = null
    },
    // Load the projection the current filter selects.
    async refresh(transport: ShellTransport): Promise<void> {
      const attempt = this.issued
      try {
        const board = await new KanbanClient(transport).queryBoardGlobal({
          filter: wireFilter(this.filter),
        })
        if (attempt !== this.issued) return
        this.cards = board.cards
        this.options = board.options
        this.loaded = true
        this.error = null
      } catch (failure) {
        if (attempt !== this.issued) return
        this.error = asApiError(failure).message
      }
    },
  },
})
