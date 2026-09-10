import { describe, expect, it } from 'vitest'
import type { ProfileRecord, RunRecord } from '@kanban/contracts'
import { effectiveFallback, plannedFallback } from './profile-fallback'

function profile(overrides: Partial<ProfileRecord> & { name: string }): ProfileRecord {
  return {
    harness: 'claude-code',
    model: 'opus',
    effort: 'high',
    usage_pool: 'operator',
    fallback: null,
    retired: false,
    version: 1,
    ...overrides,
  }
}

function snapshot(name: string) {
  return { name, harness: 'claude-code', model: 'opus', effort: 'high', usage_pool: 'operator' }
}

function run(overrides: Partial<RunRecord> = {}): RunRecord {
  return {
    id: 1,
    project_id: 1,
    ticket_id: 5,
    dispatch_request_id: 2,
    requested: snapshot('deep'),
    effective: snapshot('deep'),
    fallback: false,
    fallback_path: [],
    status: 'executing',
    created_at: 1789000000,
    version: 1,
    ...overrides,
  }
}

describe('planned fallback', () => {
  const catalogue = [
    profile({ name: 'deep', fallback: 'standard' }),
    profile({ name: 'standard', fallback: 'quick' }),
    profile({ name: 'quick' }),
    profile({ name: 'legacy', fallback: 'withdrawn' }),
    profile({ name: 'withdrawn', retired: true }),
    profile({ name: 'dangling', fallback: 'ghost' }),
    profile({ name: 'loop', fallback: 'knot' }),
    profile({ name: 'knot', fallback: 'loop' }),
  ]

  it('stops at the requested entry when the catalogue still assigns it', () => {
    expect(plannedFallback(catalogue, 'deep')).toEqual({
      chain: ['deep'],
      effective: 'deep',
      broken: null,
    })
  })

  it('crosses every retired hop, as the resolver does, to the entry that answers', () => {
    const restored = [
      profile({ name: 'alpha', fallback: 'beta', retired: true }),
      profile({ name: 'beta', fallback: 'gamma', retired: true }),
      profile({ name: 'gamma' }),
    ]

    expect(plannedFallback(restored, 'alpha')).toEqual({
      chain: ['alpha', 'beta', 'gamma'],
      effective: 'gamma',
      broken: null,
    })
  })

  it('refuses a retired entry whose policy names no successor', () => {
    expect(plannedFallback(catalogue, 'withdrawn')).toEqual({
      chain: ['withdrawn'],
      effective: null,
      broken: { name: 'withdrawn', reason: 'exhausted' },
    })
  })

  it('never reads the successor of an entry that answers', () => {
    expect(plannedFallback(catalogue, 'legacy')).toEqual({
      chain: ['legacy'],
      effective: 'legacy',
      broken: null,
    })
  })

  it('never reports an unused successor the catalogue does not hold', () => {
    expect(plannedFallback(catalogue, 'dangling')).toEqual({
      chain: ['dangling'],
      effective: 'dangling',
      broken: null,
    })
  })

  it('refuses a retired chain that ends at a name no entry carries', () => {
    const restored = [
      profile({ name: 'alpha', fallback: 'beta', retired: true }),
      profile({ name: 'beta', fallback: 'ghost', retired: true }),
    ]

    expect(plannedFallback(restored, 'alpha')).toEqual({
      chain: ['alpha', 'beta'],
      effective: null,
      broken: { name: 'ghost', reason: 'unknown' },
    })
  })

  it('never reports an unused successor that returns to the requested entry', () => {
    expect(plannedFallback(catalogue, 'loop')).toEqual({
      chain: ['loop'],
      effective: 'loop',
      broken: null,
    })
  })

  it('refuses a retired cycle, exactly as the resolver does', () => {
    const restored = [
      profile({ name: 'loop', fallback: 'knot', retired: true }),
      profile({ name: 'knot', fallback: 'loop', retired: true }),
    ]

    expect(plannedFallback(restored, 'loop')).toEqual({
      chain: ['loop', 'knot'],
      effective: null,
      broken: { name: 'loop', reason: 'cycle' },
    })
  })

  it('reports an entry the catalogue does not hold at all', () => {
    expect(plannedFallback(catalogue, 'ghost')).toEqual({
      chain: [],
      effective: null,
      broken: { name: 'ghost', reason: 'unknown' },
    })
  })
})

describe('effective fallback', () => {
  it('counts only the Runs that requested the entry', () => {
    const runs = [
      run({ id: 1 }),
      run({ id: 2, requested: snapshot('standard'), effective: snapshot('standard') }),
    ]

    expect(effectiveFallback(runs, 'deep')).toEqual({ requested: 1, fellBack: 0, paths: [] })
  })

  it('reports the walks the Runs actually recorded, once each', () => {
    const runs = [
      run({ id: 1, effective: snapshot('standard'), fallback: true, fallback_path: ['deep', 'standard'] }),
      run({ id: 2, effective: snapshot('standard'), fallback: true, fallback_path: ['deep', 'standard'] }),
      run({ id: 3, effective: snapshot('quick'), fallback: true, fallback_path: ['deep', 'standard', 'quick'] }),
      run({ id: 4 }),
    ]

    expect(effectiveFallback(runs, 'deep')).toEqual({
      requested: 4,
      fellBack: 3,
      paths: [
        ['deep', 'standard'],
        ['deep', 'standard', 'quick'],
      ],
    })
  })
})
