import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { ApiError, TimelineQueryResponse } from '@kanban/contracts'
import type { ShellTransport } from '../core/transport'
import { useTimelineStore } from './timeline'

function transportRejectingWith(failure: unknown): ShellTransport {
  return {
    query: vi.fn().mockRejectedValue(failure),
    command: vi.fn(),
    subscribe: () => () => undefined,
    onConnectionChange: () => () => undefined,
  } as unknown as ShellTransport
}

function transportAnsweringWith(response: TimelineQueryResponse): ShellTransport {
  return {
    query: vi.fn().mockResolvedValue(response),
    command: vi.fn(),
    subscribe: () => () => undefined,
    onConnectionChange: () => () => undefined,
  } as unknown as ShellTransport
}

describe('timeline store', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
  })

  it('renders a structured API refusal through the error helper', async () => {
    const refusal: ApiError = {
      code: 'invalid_request',
      message: 'a Project timeline scope must name a Project',
    }
    const store = useTimelineStore()

    await store.load(transportRejectingWith(refusal), { project: 1 })

    expect(store.error).toBe('a Project timeline scope must name a Project')
    expect(store.events).toEqual([])
  })

  it('reports an unstructured failure without losing it', async () => {
    const store = useTimelineStore()

    await store.load(transportRejectingWith(new Error('the socket closed')), 'global')

    expect(store.error).toContain('the socket closed')
  })

  it('queries the scope it was loaded with', async () => {
    const transport = transportAnsweringWith({ events: [] })
    const store = useTimelineStore()

    await store.load(transport, 'global')

    expect(transport.query).toHaveBeenCalledWith(
      'timeline.query',
      expect.objectContaining({ scope: 'global' }),
    )
    expect(store.error).toBeNull()
  })
})
