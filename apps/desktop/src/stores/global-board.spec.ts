import { createPinia, setActivePinia } from 'pinia'
import { describe, expect, it, vi } from 'vitest'
import type { BoardGlobalResponse } from '@kanban/contracts'
import type { ShellTransport } from '../core/transport'
import { useGlobalBoardStore } from './global-board'

const response = (cards: BoardGlobalResponse['cards']): BoardGlobalResponse => ({
  cards,
  options: {
    initiatives: [{ id: 1, label: 'Personal tooling' }],
    projects: [
      { id: 1, label: 'CORE — Control plane' },
      { id: 2, label: 'EDGE — Edge tooling' },
    ],
    plans: [],
    specs: [],
    lanes: [],
    profiles: ['standard'],
    attention: ['blocker', 'stale_run'],
  },
})

// A recording transport whose board answer the test steers.
function harness(answer: (request: unknown) => Promise<BoardGlobalResponse>) {
  const queries: Array<{ name: string; request: unknown }> = []
  const transport = {
    query: (name: string, request: unknown) => {
      queries.push({ name, request })
      return answer(request)
    },
    command: vi.fn(),
    subscribe: () => () => undefined,
  } as unknown as ShellTransport
  return { transport, queries }
}

function deferred<T>() {
  let settle!: (value: T) => void
  let fail!: (reason: unknown) => void
  const promise = new Promise<T>((resolve, reject) => {
    settle = resolve
    fail = reject
  })
  return { promise, settle, fail }
}

describe('global board store', () => {
  it('loads the projection through the generated client', async () => {
    setActivePinia(createPinia())
    const answer = vi.fn(() => Promise.resolve(response([])))
    const { transport, queries } = harness(answer)
    const board = useGlobalBoardStore()

    await board.refresh(transport)

    expect(queries).toEqual([{ name: 'board.global', request: { filter: {} } }])
    expect(board.options).toEqual(response([]).options)
    expect(board.loaded).toBe(true)
    expect(board.error).toBeNull()
  })

  it('carries the toggled axes into the next query', async () => {
    setActivePinia(createPinia())
    const answer = vi.fn(() => Promise.resolve(response([])))
    const { transport, queries } = harness(answer)
    const board = useGlobalBoardStore()

    board.toggleId('projects', 2)
    board.toggleWord('kinds', 'task')
    board.toggleWord('profiles', 'standard')
    await board.refresh(transport)

    expect(queries.at(-1)?.request).toEqual({
      filter: { projects: [2], kinds: ['task'], profiles: ['standard'] },
    })

    // Toggling again takes the value back out.
    board.toggleId('projects', 2)
    await board.refresh(transport)
    expect(queries.at(-1)?.request).toEqual({
      filter: { kinds: ['task'], profiles: ['standard'] },
    })
  })

  it('resets every axis at once', async () => {
    setActivePinia(createPinia())
    const answer = vi.fn(() => Promise.resolve(response([])))
    const { transport, queries } = harness(answer)
    const board = useGlobalBoardStore()
    board.toggleId('initiatives', 1)
    board.toggleWord('attention', 'blocker')

    board.resetFilter()
    await board.refresh(transport)

    expect(queries.at(-1)?.request).toEqual({ filter: {} })
  })

  it('reports a failed load without pretending to be loaded', async () => {
    setActivePinia(createPinia())
    const { transport } = harness(() =>
      Promise.reject({ code: 'unavailable', message: 'the core is offline' }),
    )
    const board = useGlobalBoardStore()

    await board.refresh(transport)

    expect(board.error).toBe('the core is offline')
    expect(board.loaded).toBe(false)
    expect(board.cards).toEqual([])
  })

  it('renders nothing from a response that outlives its clear', async () => {
    setActivePinia(createPinia())
    const slow = deferred<BoardGlobalResponse>()
    const { transport } = harness(() => slow.promise)
    const board = useGlobalBoardStore()

    const loading = board.refresh(transport)
    board.clear()
    slow.settle(response([]))
    await loading

    expect(board.loaded).toBe(false)
    expect(board.cards).toEqual([])
    expect(board.options).toBeNull()
    expect(board.error).toBeNull()
  })
})
