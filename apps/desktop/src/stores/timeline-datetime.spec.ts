import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { datetimeLocalToUtcIso } from './timeline-datetime'

describe('datetimeLocalToUtcIso', () => {
  beforeEach(() => {
    vi.stubEnv('TZ', 'UTC')
  })

  afterEach(() => {
    vi.unstubAllEnvs()
  })

  it('converts datetime-local values to UTC ISO strings with seconds', () => {
    expect(datetimeLocalToUtcIso('2026-09-04T00:00')).toBe('2026-09-04T00:00:00.000Z')
    expect(datetimeLocalToUtcIso('2026-09-04T23:59')).toBe('2026-09-04T23:59:00.000Z')
  })

  it('returns undefined for empty input', () => {
    expect(datetimeLocalToUtcIso('')).toBeUndefined()
  })
})
