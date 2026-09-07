// The saved-view state, driven entirely through the generated client:
// one `view.list` query answers every scope's views with the
// generated defaults materialised (DR-BP-06), and switching a view
// restores the whole set of properties it owns — the ten-axis filter,
// the expanded groups, the hidden columns, the mode, the Done
// placement, and the sorting key — exactly, together, every time
// (DR-BP-05). The views are per-operator data in the authoritative
// store, never browser state: editing a presentation choice writes
// through one `view.update`, so a perspective survives the window.
// Which view is active is presentation state and stays here.
import { defineStore } from 'pinia'
import { KanbanClient } from '@kanban/contracts'
import type { BoardFilter, SavedViewRecord, ViewScope } from '@kanban/contracts'
import { asApiError } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import {
  DEFAULT_BOARD_PRESENTATION,
  DEFAULT_DONE_PRESENTATION,
  DEFAULT_HIDDEN_COLUMNS,
} from '../views/board-layout'
import { emptyFilter } from '../views/global-board-filters'
import { useGlobalBoardStore } from './global-board'

/** The Project a scope names, or null for the global scope. */
export function scopeProjectId(scope: ViewScope): number | null {
  return scope === 'global' ? null : scope.project
}

/** Whether a view belongs to one scope. */
export function inScope(view: SavedViewRecord, scope: ViewScope): boolean {
  const project = scopeProjectId(scope)
  return scopeProjectId(view.scope) === project
}

/** The whole set of presentation properties one view owns, with the
 * filter held whole rather than as the sparse shape the wire
 * serialises empty axes away into. */
export type ViewOwnedSet = {
  filter: BoardFilter
  expanded_groups: SavedViewRecord['expanded_groups']
  hidden_columns: SavedViewRecord['hidden_columns']
  mode: SavedViewRecord['mode']
  done_placement: SavedViewRecord['done_placement']
  sorting: SavedViewRecord['sorting']
}

/** The everyday perspective, as the generated defaults carry it: the
 * set a board falls back on while no view of its scope has loaded. */
export function fallbackOwnedSet(): ViewOwnedSet {
  return {
    filter: emptyFilter(),
    expanded_groups: [],
    hidden_columns: [...DEFAULT_HIDDEN_COLUMNS],
    mode: DEFAULT_BOARD_PRESENTATION,
    done_placement: DEFAULT_DONE_PRESENTATION,
    sorting: 'priority',
  }
}

/** One owned set as an independent copy, so later edits of the record
 * never leak into a state a switch already applied. */
export function ownedCopy(view: SavedViewRecord): ViewOwnedSet {
  return {
    filter: { ...emptyFilter(), ...view.filter },
    expanded_groups: [...view.expanded_groups],
    hidden_columns: [...view.hidden_columns],
    mode: view.mode,
    done_placement: view.done_placement,
    sorting: view.sorting,
  }
}

export const useSavedViewsStore = defineStore('saved-views', {
  state: () => ({
    /** Every view of every scope, defaults included. */
    views: [] as SavedViewRecord[],
    /** The view the global board rests on; null until views load. */
    activeGlobalViewId: null as number | null,
    /** The view each Project's board rests on, by Project. */
    activeProjectViewIds: {} as Record<number, number>,
    loaded: false,
    error: null as string | null,
  }),
  getters: {
    /** The global scope's views, default first. */
    globalViews(state): SavedViewRecord[] {
      return state.views.filter((view) => scopeProjectId(view.scope) === null)
    },
    /** One Project's views, default first. */
    projectViews(state): (projectId: number) => SavedViewRecord[] {
      return (projectId: number) =>
        state.views.filter((view) => scopeProjectId(view.scope) === projectId)
    },
    /** One view by identity, when it stands. */
    viewOf(state): (viewId: number | null) => SavedViewRecord | null {
      return (viewId: number | null) =>
        state.views.find((view) => view.id === viewId) ?? null
    },
    /** The view the global board rests on. */
    activeGlobalView(): SavedViewRecord | null {
      return this.viewOf(this.activeGlobalViewId)
    },
    /** The view one Project's board rests on. */
    activeProjectView(): (projectId: number) => SavedViewRecord | null {
      return (projectId: number) => this.viewOf(this.activeProjectViewIds[projectId] ?? null)
    },
  },
  actions: {
    // Load every scope's views and seed each scope's active view with
    // its generated default when nothing is chosen yet.
    async refresh(transport: ShellTransport): Promise<void> {
      try {
        const response = await new KanbanClient(transport).queryViewList({})
        this.views = response.views
        if (this.activeGlobalViewId === null) {
          this.activeGlobalViewId = this.globalViews.find((view) => view.is_default)?.id ?? null
        }
        for (const view of response.views) {
          const project = scopeProjectId(view.scope)
          if (project !== null && view.is_default && this.activeProjectViewIds[project] === undefined) {
            this.activeProjectViewIds[project] = view.id
          }
        }
        this.loaded = true
        this.error = null
      } catch (failure) {
        this.error = asApiError(failure).message
      }
    },
    // Switch the global board to one view, restoring every property
    // it owns: the filter lands in the global board store whole —
    // every axis, present or empty — and the presentation properties
    // ride the active view the boards read.
    switchGlobalView(viewId: number): boolean {
      const view = this.viewOf(viewId)
      if (view === null) return false
      this.activeGlobalViewId = viewId
      useGlobalBoardStore().setFilter(ownedCopy(view).filter)
      return true
    },
    // Switch one Project's board to one of that scope's views.
    switchProjectView(projectId: number, viewId: number): boolean {
      const view = this.viewOf(viewId)
      if (view === null || scopeProjectId(view.scope) !== projectId) return false
      this.activeProjectViewIds[projectId] = viewId
      return true
    },
    // Write one presentation change through to the view that owns
    // it: the whole owned set travels, so a property the caller did
    // not name keeps its value and nothing is dropped.
    async reviseOwnedSet(
      transport: ShellTransport,
      viewId: number,
      changes: Partial<ViewOwnedSet>,
    ): Promise<void> {
      const view = this.viewOf(viewId)
      if (view === null) return
      const owned = { ...ownedCopy(view), ...changes }
      try {
        const updated = await new KanbanClient(transport).commandViewUpdate({
          mutation: {
            optimistic_version: view.version,
            idempotency_key: crypto.randomUUID(),
          },
          view_id: viewId,
          ...owned,
        })
        this.views = this.views.map((entry) => (entry.id === updated.id ? updated : entry))
        this.error = null
      } catch (failure) {
        // The record the store still holds stands; the board
        // renders it until a change lands.
        this.error = asApiError(failure).message
      }
    },
    // Name and keep the perspective the boards currently hold.
    async createView(
      transport: ShellTransport,
      name: string,
      scope: ViewScope,
      owned: ViewOwnedSet,
    ): Promise<SavedViewRecord | null> {
      try {
        const created = await new KanbanClient(transport).commandViewCreate({
          mutation: {
            optimistic_version: 0,
            idempotency_key: crypto.randomUUID(),
          },
          scope,
          name,
          ...owned,
        })
        this.views = [...this.views, created]
        this.error = null
        return created
      } catch (failure) {
        this.error = asApiError(failure).message
        return null
      }
    },
  },
})
