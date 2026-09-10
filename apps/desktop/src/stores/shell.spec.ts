import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { AttentionListResponse } from '@kanban/contracts'
import type { ShellTransport } from '../core/transport'
import { usePreferencesStore } from './preferences'
import { useShellStore } from './shell'

function harness(items: AttentionListResponse['items']) {
  const query = vi.fn((name: string) => {
    if (name === 'attention.list') return Promise.resolve({ items })
    throw new Error(`unexpected query ${name}`)
  })
  const transport = {
    query,
    command: vi.fn(),
    subscribe: () => () => undefined,
    onConnectionChange: () => () => undefined,
  } as unknown as ShellTransport
  return { transport, query }
}

const item = (overrides: Partial<AttentionListResponse['items'][number]>) => ({
  id: 'a1',
  kind: 'blocker' as const,
  project_id: 1,
  subject_kind: 'ticket' as const,
  subject_id: '7',
  summary: 'KAN-T7 blocked',
  detail: {},
  active: true,
  first_seen_at: '2026-09-13T10:00:00Z',
  last_seen_at: '2026-09-13T10:00:00Z',
  acknowledged_at: null,
  acknowledged_by: null,
  version: 1,
  ...overrides,
})

describe('shell store', () => {
  beforeEach(() => {
    localStorage.clear()
    setActivePinia(createPinia())
  })

  it('shows the rail as the arrangement the core holds says', () => {
    const shell = useShellStore()
    const preferences = usePreferencesStore()
    expect(shell.railExpanded).toBe(true)

    preferences.adopt({ rail_open: false, collapsed_columns: [], version: 1 })

    expect(shell.railExpanded).toBe(false)
    // The rail preference is the core's record, not this store's.
    expect(localStorage.length).toBe(0)
  })

  it('keeps only the icons at narrow width without touching the rail preference', () => {
    const shell = useShellStore()
    const preferences = usePreferencesStore()
    expect(shell.railExpanded).toBe(true)
    shell.setNarrow(true)
    expect(shell.railExpanded).toBe(false)
    expect(preferences.railOpen).toBe(true)
    shell.setNarrow(false)
    expect(shell.railExpanded).toBe(true)
  })

  it('counts the attention items still waiting on the operator', async () => {
    const { transport, query } = harness([
      item({ id: 'a1' }),
      item({ id: 'a2', acknowledged_by: 'sid', acknowledged_at: '2026-09-13T11:00:00Z' }),
      item({ id: 'a3', active: false }),
      item({ id: 'a4', kind: 'stale_run' }),
    ])
    const shell = useShellStore()

    await shell.refreshAttention(transport)

    expect(query).toHaveBeenCalledWith('attention.list', {})
    expect(shell.attentionCount).toBe(2)
  })

  it('shows no count rather than a wrong one when the inbox cannot be read', async () => {
    const transport = {
      query: () => Promise.reject({ code: 'unavailable', message: 'the core is offline' }),
      command: vi.fn(),
      subscribe: () => () => undefined,
      onConnectionChange: () => () => undefined,
    } as unknown as ShellTransport
    const shell = useShellStore()

    await shell.refreshAttention(transport)

    expect(shell.attentionCount).toBeNull()
  })
})
