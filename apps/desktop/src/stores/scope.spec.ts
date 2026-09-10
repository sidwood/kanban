import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it } from 'vitest'
import { boardRouteFor, scopeKeyOf, scopeOfRoute, useScopeStore } from './scope'
import { coreProject, edgeProject } from '../test/shell-harness'

describe('board scope', () => {
  beforeEach(() => {
    localStorage.clear()
    setActivePinia(createPinia())
  })

  it('opens on every Project until the operator narrows it', () => {
    expect(useScopeStore().scope).toBe('all')
  })

  it('routes each scope to its board', () => {
    expect(boardRouteFor('all')).toBe('/board')
    expect(boardRouteFor(4)).toBe('/projects/4/board')
  })

  it('reads the scope a board route names', () => {
    expect(scopeOfRoute({ projectId: '4' })).toBe(4)
    expect(scopeOfRoute({})).toBe('all')
    expect(scopeOfRoute({ projectId: 'x' })).toBeNull()
  })

  it('keys each scope for the state that hangs off it', () => {
    expect(scopeKeyOf('all')).toBe('global')
    expect(scopeKeyOf(7)).toBe('project:7')
  })

  it('opens on every Project again after a reload, keeping nothing in storage', () => {
    const scope = useScopeStore()
    scope.set(3)
    expect(localStorage.length).toBe(0)

    setActivePinia(createPinia())
    expect(useScopeStore().scope).toBe('all')
  })

  it('falls back to every Project when the scoped Project is gone or archived', () => {
    const scope = useScopeStore()
    scope.set(2)
    scope.reconcile([coreProject, edgeProject])
    expect(scope.scope).toBe(2)

    scope.reconcile([coreProject, { ...edgeProject, archived: true }])
    expect(scope.scope).toBe('all')

    scope.set(9)
    scope.reconcile([coreProject, edgeProject])
    expect(scope.scope).toBe('all')
  })

  it('adopts the scope a board route arrived on', () => {
    const scope = useScopeStore()
    scope.adoptRoute({ projectId: '9' })
    expect(scope.scope).toBe(9)
    scope.adoptRoute({})
    expect(scope.scope).toBe('all')
  })
})
