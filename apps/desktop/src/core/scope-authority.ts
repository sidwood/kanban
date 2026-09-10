// Reads are ordered within one authority state. Use separate states when a
// collection and its selected object have independent lifetimes.
export interface ScopeState {
  scopeKey: string | null
  scopeGeneration: number
  readSequence: number
}

export interface ScopeClaim {
  readonly key: string | null
  readonly generation: number
  readonly read: number | null
}

export function emptyScope(): ScopeState {
  return { scopeKey: null, scopeGeneration: 0, readSequence: 0 }
}

export function projectScopeKey(projectId: number | null): string {
  return projectId === null ? 'project:none' : `project:${projectId}`
}

export function adoptScope(state: ScopeState, key: string): ScopeClaim {
  if (state.scopeKey !== key) {
    state.scopeGeneration += 1
    state.scopeKey = key
    state.readSequence = 0
  }
  state.readSequence += 1
  return { key, generation: state.scopeGeneration, read: state.readSequence }
}

export function issueRead(state: ScopeState): ScopeClaim {
  state.readSequence += 1
  return { key: state.scopeKey, generation: state.scopeGeneration, read: state.readSequence }
}

// Commands survive unrelated reads, but never a scope change.
export function issueCommand(state: ScopeState): ScopeClaim {
  return { key: state.scopeKey, generation: state.scopeGeneration, read: null }
}

export function scopeHolds(state: ScopeState, claim: ScopeClaim): boolean {
  if (claim.key !== state.scopeKey || claim.generation !== state.scopeGeneration) return false
  return claim.read === null || claim.read === state.readSequence
}

export function scopeIs(state: ScopeState, key: string): boolean {
  return state.scopeKey === key
}

export function releaseScope(state: ScopeState): void {
  state.scopeGeneration += 1
  state.scopeKey = null
  state.readSequence = 0
}
