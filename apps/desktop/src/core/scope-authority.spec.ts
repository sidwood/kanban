import { describe, expect, it } from 'vitest'
import {
  adoptScope,
  emptyScope,
  issueCommand,
  issueRead,
  projectScopeKey,
  releaseScope,
  scopeHolds,
  scopeIs,
} from './scope-authority'

describe('scope authority', () => {
  it('holds the scope it last took', () => {
    const state = emptyScope()

    expect(scopeIs(state, projectScopeKey(1))).toBe(false)
    adoptScope(state, projectScopeKey(1))

    expect(scopeIs(state, projectScopeKey(1))).toBe(true)
    expect(scopeIs(state, projectScopeKey(2))).toBe(false)
  })

  it('supersedes every read and command of the scope it leaves', () => {
    const state = emptyScope()
    const read = adoptScope(state, projectScopeKey(1))
    const command = issueCommand(state)

    adoptScope(state, projectScopeKey(2))

    expect(scopeHolds(state, read)).toBe(false)
    expect(scopeHolds(state, command)).toBe(false)
  })

  it('never accepts a claim minted by a different keyed authority', () => {
    const first = emptyScope()
    const second = emptyScope()
    const claim = adoptScope(first, 'plan:1')
    adoptScope(second, 'plan:2')

    expect(scopeHolds(second, claim)).toBe(false)
  })

  it('keeps only the latest read of the scope it holds', () => {
    const state = emptyScope()
    const first = adoptScope(state, projectScopeKey(1))
    const second = issueRead(state)

    expect(scopeHolds(state, first)).toBe(false)
    expect(scopeHolds(state, second)).toBe(true)
  })

  it('never lets a re-read supersede a command still in flight', () => {
    const state = emptyScope()
    adoptScope(state, projectScopeKey(1))
    const command = issueCommand(state)

    adoptScope(state, projectScopeKey(1))
    issueRead(state)

    expect(scopeHolds(state, command)).toBe(true)
  })

  it('supersedes everything when the scope is given up', () => {
    const state = emptyScope()
    const read = adoptScope(state, projectScopeKey(1))
    const command = issueCommand(state)

    releaseScope(state)

    expect(scopeHolds(state, read)).toBe(false)
    expect(scopeHolds(state, command)).toBe(false)
    expect(state.scopeKey).toBeNull()
  })

  it('refuses a claim from a scope taken again after being given up', () => {
    const state = emptyScope()
    adoptScope(state, projectScopeKey(1))
    const command = issueCommand(state)
    releaseScope(state)
    adoptScope(state, projectScopeKey(1))

    expect(scopeHolds(state, command)).toBe(false)
  })
})
