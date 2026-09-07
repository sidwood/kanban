import { createPinia, setActivePinia } from 'pinia'
import { describe, expect, it, vi } from 'vitest'
import type { SearchGlobalResponse } from '@kanban/contracts'
import type { ShellTransport } from '../core/transport'
import {
  PALETTE_NAVIGATION,
  filterNavigation,
  mergePaletteItems,
  paletteItemFromHit,
  routeForSearchHit,
} from './palette-navigation'
import { usePaletteStore } from './palette'
import { useSearchStore } from './search'

function harness(answer: (request: unknown) => Promise<SearchGlobalResponse>) {
  const queries: Array<{ name: string; request: unknown }> = []
  const commands: Array<{ name: string; request: unknown }> = []
  const transport = {
    query: (name: string, request: unknown) => {
      queries.push({ name, request })
      return answer(request)
    },
    command: (name: string, request: unknown) => {
      commands.push({ name, request })
      return Promise.reject(new Error('commands are forbidden here'))
    },
    subscribe: () => () => undefined,
  } as unknown as ShellTransport
  return { transport, queries, commands }
}

describe('palette navigation', () => {
  it('filters navigation rows by label', () => {
    expect(filterNavigation('board').map((item) => item.id)).toEqual(['nav-board'])
    expect(filterNavigation('')).toEqual([...PALETTE_NAVIGATION])
  })

  it('maps every search kind to a route', () => {
    expect(routeForSearchHit({
      kind: 'ticket',
      id: 2,
      identifier: 'CORE-T2',
      label: 'Archive the register',
      project_id: 1,
    })).toBe('/projects/1/board')
    expect(paletteItemFromHit({
      kind: 'spec',
      id: 3,
      identifier: 'CORE-S3',
      label: 'Board presentation',
      project_id: 1,
    }).route).toBe('/planning/specs')
  })

  it('places navigation before search hits', () => {
    const items = mergePaletteItems(filterNavigation(''), [
      {
        kind: 'project',
        id: 1,
        identifier: 'CORE',
        label: 'Control plane',
        project_id: 1,
      },
    ])
    expect(items[0].kind).toBe('navigation')
    expect(items.at(-1)?.kind).toBe('project')
  })
})

describe('palette store', () => {
  it('composes navigation and search hits', async () => {
    setActivePinia(createPinia())
    const { transport } = harness(() =>
      Promise.resolve({
        hits: [
          {
            kind: 'plan',
            id: 1,
            identifier: 'CORE-P1',
            label: 'Plan 1',
            project_id: 1,
          },
        ],
      }),
    )
    const palette = usePaletteStore()
    palette.openPalette()

    await palette.setQuery(transport, 'plan')

    expect(palette.items.some((item) => item.kind === 'plan')).toBe(true)
    expect(palette.items.some((item) => item.kind === 'navigation')).toBe(true)
  })

  it('issues only search queries and never commands', async () => {
    setActivePinia(createPinia())
    const { transport, queries, commands } = harness(() =>
      Promise.resolve({
        hits: [
          {
            kind: 'plan',
            id: 1,
            identifier: 'CORE-P1',
            label: 'Plan 1',
            project_id: 1,
          },
        ],
      }),
    )
    const palette = usePaletteStore()
    palette.openPalette()

    await palette.setQuery(transport, 'core-p1')
    palette.moveSelection(1)
    palette.selectedItem()

    expect(queries).toEqual([{ name: 'search.global', request: { q: 'core-p1' } }])
    expect(commands).toEqual([])
  })

  it('clears search state when it closes', async () => {
    setActivePinia(createPinia())
    const answer = vi.fn(() => Promise.resolve({ hits: [] }))
    const { transport } = harness(answer)
    const palette = usePaletteStore()
    const search = useSearchStore()

    palette.openPalette()
    await palette.setQuery(transport, 'archive')
    palette.closePalette()

    expect(search.query).toBe('')
    expect(search.hits).toEqual([])
    expect(palette.open).toBe(false)
  })
})
