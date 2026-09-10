// The board scope the shell holds: every Project, or one of them.
// Session state, never stored: the board routes carry it, so a link
// lands on the scope it names, and a fresh window opens on every
// Project rather than on a Project that may since have gone.
import { defineStore } from 'pinia'
import type { ProjectRecord } from '@kanban/contracts'

export type BoardScope = 'all' | number

/** The key the per-scope state hangs off: Saved View working sets,
 * column collapse, and the like. */
export type ScopeKey = 'global' | `project:${number}`

export function scopeKeyOf(scope: BoardScope): ScopeKey {
  return scope === 'all' ? 'global' : `project:${scope}`
}

/** The board route one scope opens. */
export function boardRouteFor(scope: BoardScope): string {
  return scope === 'all' ? '/board' : `/projects/${scope}/board`
}

/** The scope a board route names: every Project when the route
 * carries no Project, null when it carries one that is not a
 * number. */
export function scopeOfRoute(params: {
  projectId?: string | string[] | undefined
}): BoardScope | null {
  const raw = Array.isArray(params.projectId) ? params.projectId[0] : params.projectId
  if (raw === undefined || raw === '') return 'all'
  const id = Number(raw)
  return Number.isInteger(id) && id > 0 ? id : null
}

export const useScopeStore = defineStore('scope', {
  state: () => ({
    scope: 'all' as BoardScope,
  }),
  getters: {
    key(state): ScopeKey {
      return scopeKeyOf(state.scope)
    },
    projectId(state): number | null {
      return state.scope === 'all' ? null : state.scope
    },
  },
  actions: {
    set(scope: BoardScope): void {
      this.scope = scope
    },
    // A scope naming a Project the register no longer offers — gone
    // or archived — falls back to every Project.
    reconcile(projects: readonly ProjectRecord[]): void {
      if (this.scope === 'all') return
      const project = projects.find((entry) => entry.id === this.scope)
      if (project === undefined || project.archived) this.scope = 'all'
    },
    // A board route names its scope; arriving on it adopts it.
    adoptRoute(params: { projectId?: string | string[] | undefined }): void {
      const scope = scopeOfRoute(params)
      if (scope !== null) this.set(scope)
    },
  },
})
