import { createPinia, setActivePinia } from 'pinia'
import { describe, expect, it, vi } from 'vitest'
import type { SearchGlobalResponse } from '@kanban/contracts'
import type { ShellTransport } from '../core/transport'
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

describe('search store', () => {
  it('loads hits through the generated client', async () => {
    setActivePinia(createPinia())
    const { transport, queries } = harness(() =>
      Promise.resolve({
        hits: [
          {
            kind: 'ticket',
            id: 1,
            identifier: 'CORE-T1',
            label: 'Archive the register',
            project_id: 1,
          },
        ],
      }),
    )
    const search = useSearchStore()
    search.setQuery('archive')

    await search.refresh(transport)

    expect(queries).toEqual([{ name: 'search.global', request: { q: 'archive' } }])
    expect(search.hits).toHaveLength(1)
    expect(search.error).toBeNull()
  })

  it('skips the query when the text is blank', async () => {
    setActivePinia(createPinia())
    const answer = vi.fn(() => Promise.resolve({ hits: [] }))
    const { transport, queries } = harness(answer)
    const search = useSearchStore()

    await search.refresh(transport)

    expect(queries).toEqual([])
    expect(answer).not.toHaveBeenCalled()
    expect(search.hits).toEqual([])
  })
})
