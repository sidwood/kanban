import { createPinia, setActivePinia } from 'pinia'
import { flushPromises } from '@vue/test-utils'
import { describe, expect, it, vi } from 'vitest'
import type { HealthResponse, KanbanLiveEvent } from '@kanban/contracts'
import type { ShellConnectionState, ShellTransport } from '../core/transport'
import { useConnectionStore } from './connection'

// A controllable transport: the health answers and the event and
// connection deliveries are all steerable from the test.
function harness() {
  const queries = vi.fn()
  let eventHandler: ((event: KanbanLiveEvent) => void) | undefined
  let connectionHandler: ((state: ShellConnectionState) => void) | undefined
  const transport = {
    query: (name: string, request: unknown) => queries(name, request),
    command: () => Promise.reject(new Error('no commands are catalogued yet')),
    subscribe(handler: (event: KanbanLiveEvent) => void) {
      eventHandler = handler
      return () => undefined
    },
    onConnectionChange(handler: (state: ShellConnectionState) => void) {
      connectionHandler = handler
      return () => undefined
    },
  } as unknown as ShellTransport
  return {
    transport,
    queries,
    healthy(version: string) {
      queries.mockImplementation(() =>
        Promise.resolve({ connected: true, service_version: version } satisfies HealthResponse),
      )
    },
    unreachable() {
      queries.mockImplementation(() =>
        Promise.reject(new Error('the core is unreachable')),
      )
    },
    emitEvent(sequence: number) {
      eventHandler?.({
        sequence,
        event_type: 'initiative.created',
        payload: { id: 1, name: 'Alpha', archived: false, version: 1 },
      })
    },
    announce(state: ShellConnectionState) {
      connectionHandler?.(state)
    },
  }
}

describe('connection store', () => {
  it('connects through the generated client and keeps the version', async () => {
    setActivePinia(createPinia())
    const { transport, healthy, queries } = harness()
    healthy('0.42.0')
    const connection = useConnectionStore()

    await connection.boot(transport)
    await flushPromises()

    expect(connection.phase).toBe('connected')
    expect(connection.serviceVersion).toBe('0.42.0')
    expect(queries).toHaveBeenCalledWith('health.get', {})
  })

  it('reports a failing core as unreachable', async () => {
    setActivePinia(createPinia())
    const { transport, unreachable } = harness()
    unreachable()
    const connection = useConnectionStore()

    await connection.boot(transport)
    await flushPromises()

    expect(connection.phase).toBe('disconnected')
    expect(connection.serviceVersion).toBeNull()
  })

  it('tracks the ordered event stream sequence', async () => {
    setActivePinia(createPinia())
    const { transport, healthy, emitEvent } = harness()
    healthy('0.1.0')
    const connection = useConnectionStore()

    await connection.boot(transport)
    expect(connection.lastEventSequence).toBeNull()

    emitEvent(4)
    emitEvent(5)
    expect(connection.lastEventSequence).toBe(5)
  })

  it('re-verifies through the client on every connection announcement', async () => {
    setActivePinia(createPinia())
    const { transport, healthy, unreachable, announce } = harness()
    healthy('0.1.0')
    const connection = useConnectionStore()
    await connection.boot(transport)
    expect(connection.phase).toBe('connected')

    unreachable()
    announce('disconnected')
    await flushPromises()
    expect(connection.phase).toBe('disconnected')

    healthy('0.2.0')
    announce('connected')
    await flushPromises()
    expect(connection.phase).toBe('connected')
    expect(connection.serviceVersion).toBe('0.2.0')
  })

  it('boots only once', async () => {
    setActivePinia(createPinia())
    const { transport, healthy, queries } = harness()
    healthy('0.1.0')
    const connection = useConnectionStore()

    await connection.boot(transport)
    await connection.boot(transport)
    await flushPromises()

    expect(queries).toHaveBeenCalledTimes(1)
  })
})
