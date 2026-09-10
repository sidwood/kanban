// The saved-view state, driven entirely through the generated client:
// one `view.list` query answers every scope's views with the
// generated defaults materialised (DR-BP-06), and switching a view
// restores the whole set of properties it owns — the ten-axis filter,
// the expanded groups, the hidden columns, the mode, the Done
// placement, and the sorting key — exactly, together, every time
// (DR-BP-05). The views are per-operator data in the authoritative
// store, never browser state: a board edits a working copy of the
// owned set, the drift between that copy and the record is what the
// Save and Reset controls answer to, and saving writes the whole set
// through one `view.update`, so a perspective survives the window.
// Which view is active, and the unsaved working copy, are
// presentation state and stay here.
import { defineStore } from 'pinia'
import { KanbanClient } from '@kanban/contracts'
import type { BoardFilter, SavedViewRecord, ViewScope } from '@kanban/contracts'
import { asApiError } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import {
  DEFAULT_BOARD_PRESENTATION,
  DEFAULT_DONE_PRESENTATION,
  DEFAULT_HIDDEN_COLUMNS,
  canonicalGroups,
} from '../views/board-layout'
import { emptyFilter } from '../views/global-board-filters'
import type { ScopeKey } from './scope'

/** The Project a scope names, or null for the global scope. */
export function scopeProjectId(scope: ViewScope): number | null {
  return scope === 'global' ? null : scope.project
}

/** Whether a view belongs to one scope. */
export function inScope(view: SavedViewRecord, scope: ViewScope): boolean {
  const project = scopeProjectId(scope)
  return scopeProjectId(view.scope) === project
}

/** The Project a scope key names, or null for the global scope. */
function projectOfKey(key: ScopeKey): number | null {
  return key === 'global' ? null : Number(key.slice('project:'.length))
}

/** The wire scope a scope key names. */
function wireScopeOf(key: ScopeKey): ViewScope {
  const project = projectOfKey(key)
  return project === null ? 'global' : { project }
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
    expanded_groups: canonicalGroups(view.expanded_groups),
    hidden_columns: canonicalGroups(view.hidden_columns),
    mode: view.mode,
    done_placement: view.done_placement,
    sorting: view.sorting,
  }
}

/** One owned set as a Project scope holds it: the Project axis is
 * the scope itself, never a choice, so it always names the scope's
 * Project exactly — the convention the generated Project default
 * already follows. The global scope keeps the axis as recorded. */
export function pinnedToKey(key: ScopeKey, owned: ViewOwnedSet): ViewOwnedSet {
  const project = projectOfKey(key)
  if (project === null) return owned
  return { ...owned, filter: { ...owned.filter, projects: [project] } }
}

function sameValues(a: readonly unknown[] | undefined, b: readonly unknown[] | undefined): boolean {
  const left = a ?? []
  const right = b ?? []
  return left.length === right.length && left.every((value, index) => value === right[index])
}

/** Whether two owned sets say the same thing, axis by axis. */
export function sameOwnedSet(a: ViewOwnedSet, b: ViewOwnedSet): boolean {
  const axes = Object.keys(emptyFilter()) as (keyof BoardFilter)[]
  return (
    a.mode === b.mode &&
    a.done_placement === b.done_placement &&
    a.sorting === b.sorting &&
    sameValues(a.expanded_groups, b.expanded_groups) &&
    sameValues(a.hidden_columns, b.hidden_columns) &&
    axes.every((axis) => sameValues(a.filter[axis], b.filter[axis]))
  )
}

export const useSavedViewsStore = defineStore('saved-views', {
  state: () => ({
    /** Every view of every scope, defaults included. */
    views: [] as SavedViewRecord[],
    /** The view the global board rests on; null until views load. */
    activeGlobalViewId: null as number | null,
    /** The view each Project's board rests on, by Project. */
    activeProjectViewIds: {} as Record<number, number>,
    /** The working copy each scope's board edits; absent while the
     * board rests on its record. */
    working: {} as Partial<Record<ScopeKey, ViewOwnedSet>>,
    /** How many times each scope's working copy has changed hands —
     * revised, reset, or switched away from. A save carries one
     * revision, so a response that lands after the operator has
     * moved on can tell that it no longer speaks for the board. */
    revisions: {} as Partial<Record<ScopeKey, number>>,
    loaded: false,
    error: null as string | null,
  }),
  getters: {
    /** The global scope's views, default first. */
    globalViews(state): SavedViewRecord[] {
      return state.views.filter((view) => scopeProjectId(view.scope) === null)
    },
    /** One scope's views, default first. */
    viewsFor(state): (key: ScopeKey) => SavedViewRecord[] {
      return (key: ScopeKey) => {
        const project = projectOfKey(key)
        return state.views.filter((view) => scopeProjectId(view.scope) === project)
      }
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
    /** The view one scope's board rests on. */
    activeViewFor(): (key: ScopeKey) => SavedViewRecord | null {
      return (key: ScopeKey) => {
        const project = projectOfKey(key)
        return project === null ? this.activeGlobalView : this.activeProjectView(project)
      }
    },
    /** The owned set one scope's board rests on: its record, pinned
     * to the scope, or the everyday perspective. */
    restingFor(): (key: ScopeKey) => ViewOwnedSet {
      return (key: ScopeKey) => {
        const view = this.activeViewFor(key)
        return pinnedToKey(key, view ? ownedCopy(view) : fallbackOwnedSet())
      }
    },
    /** The owned set one scope's board renders: its working copy
     * while one stands, else the set it rests on. */
    workingFor(): (key: ScopeKey) => ViewOwnedSet {
      return (key: ScopeKey) => this.working[key] ?? this.restingFor(key)
    },
    /** Whether one scope's working copy has drifted from its record. */
    isDrifted(): (key: ScopeKey) => boolean {
      return (key: ScopeKey) => {
        const working = this.working[key]
        if (working === undefined) return false
        return !sameOwnedSet(working, this.restingFor(key))
      }
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
    // Switch one scope's board to one of the scope's views; the
    // working copy goes with the switch, so the board rests on the
    // chosen record exactly (DR-BP-05).
    switchView(key: ScopeKey, viewId: number): boolean {
      if (!this.restOn(key, viewId)) return false
      delete this.working[key]
      this.revisions[key] = (this.revisions[key] ?? 0) + 1
      return true
    },
    // Rest one scope's board on one of the scope's views without
    // touching the working copy the operator is editing.
    restOn(key: ScopeKey, viewId: number): boolean {
      const view = this.viewOf(viewId)
      const project = projectOfKey(key)
      if (view === null || scopeProjectId(view.scope) !== project) return false
      if (project === null) {
        this.activeGlobalViewId = viewId
      } else {
        this.activeProjectViewIds[project] = viewId
      }
      return true
    },
    // Edit one scope's working copy: the board changes at once, the
    // record stands until the operator saves.
    revise(key: ScopeKey, changes: Partial<ViewOwnedSet>): void {
      const next = { ...this.workingFor(key), ...changes }
      this.working[key] = pinnedToKey(key, {
        ...next,
        expanded_groups: canonicalGroups(next.expanded_groups),
        hidden_columns: canonicalGroups(next.hidden_columns),
      })
      this.revisions[key] = (this.revisions[key] ?? 0) + 1
    },
    // Drop one scope's working copy: the board returns to its record.
    resetWorking(key: ScopeKey): void {
      delete this.working[key]
      this.revisions[key] = (this.revisions[key] ?? 0) + 1
    },
    // Write one scope's working copy through to the view it rests
    // on: the whole owned set travels, and the record that comes back
    // is the one the board then rests on — unless the operator has
    // edited the perspective or changed view since, in which case
    // what they hold now is newer than what was saved and stands as
    // drift over the record (KAN-T137-AC4).
    async saveWorking(transport: ShellTransport, key: ScopeKey): Promise<void> {
      const view = this.activeViewFor(key)
      if (view === null) return
      const owned = this.workingFor(key)
      const submitted = this.revisions[key] ?? 0
      try {
        const updated = await new KanbanClient(transport).commandViewUpdate({
          mutation: {
            optimistic_version: view.version,
            idempotency_key: crypto.randomUUID(),
          },
          view_id: view.id,
          ...owned,
        })
        this.views = this.views.map((entry) => (entry.id === updated.id ? updated : entry))
        if (this.settled(key, submitted, view.id)) delete this.working[key]
        this.error = null
      } catch (failure) {
        // The working copy stands; the operator can retry or reset.
        this.error = asApiError(failure).message
      }
    },
    // Keep one scope's working copy as a new named view of the scope
    // and rest the board on it. An edit made while the view was being
    // created is newer than the record and stays as drift over it.
    // The record itself is always kept — the operator asked for it by
    // name — but what the board rests on is theirs to choose, so a
    // view or a reset chosen while the creation was on the wire
    // stands over this one (KAN-T137-AC4).
    async saveWorkingAs(
      transport: ShellTransport,
      key: ScopeKey,
      name: string,
    ): Promise<SavedViewRecord | null> {
      const submitted = this.revisions[key] ?? 0
      const origin = this.activeViewFor(key)?.id ?? null
      const created = await this.createView(transport, name, wireScopeOf(key), this.workingFor(key))
      if (created === null) return null
      if (!this.mayRestOn(key, origin, created)) return created
      this.restOn(key, created.id)
      if ((this.revisions[key] ?? 0) === submitted) delete this.working[key]
      return created
    },
    /** Whether the board may rest one scope on a view just created
     * from it: the operator is still on the view it was created from,
     * and resting on it leaves the perspective they are holding
     * exactly as it is. A working copy stands over the record either
     * way, so resting under one changes nothing on screen; with none
     * standing, the record itself is what the board would show, and
     * only a record saying what the board already says may take
     * over. */
    mayRestOn(key: ScopeKey, origin: number | null, created: SavedViewRecord): boolean {
      if ((this.activeViewFor(key)?.id ?? null) !== origin) return false
      if (this.working[key] !== undefined) return true
      return sameOwnedSet(this.workingFor(key), pinnedToKey(key, ownedCopy(created)))
    },
    /** Whether the perspective a save carried is still the one the
     * board holds: no later edit, and the same view underneath. */
    settled(key: ScopeKey, submitted: number, viewId: number): boolean {
      return (this.revisions[key] ?? 0) === submitted && this.activeViewFor(key)?.id === viewId
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
