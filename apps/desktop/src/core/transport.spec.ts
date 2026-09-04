import { describe, expect, it, vi } from 'vitest'

// The Tauri IPC bridge, faked at the module boundary so the adapter
// is proven on its own.
const { invokeMock, listenMock } = vi.hoisted(() => ({
  invokeMock: vi.fn(),
  listenMock: vi.fn(),
}))
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }))
vi.mock('@tauri-apps/api/event', () => ({ listen: listenMock }))

import { asApiError, tauriTransport } from './transport'

// Capture what listen was handed, per event name.
function captureListeners() {
  const handlers = new Map<string, (event: { payload: unknown }) => void>()
  listenMock.mockImplementation(
    (name: string, handler: (event: { payload: unknown }) => void) => {
      handlers.set(name, handler)
      return Promise.resolve(vi.fn())
    },
  )
  return handlers
}

describe('tauri transport', () => {
  it('routes a generated query to its typed shell command', async () => {
    invokeMock.mockResolvedValueOnce({ connected: true, service_version: '0.1.0' })

    const health = await tauriTransport.query<
      Record<string, never>,
      { connected: boolean; service_version: string }
    >('health.get', {})

    expect(invokeMock).toHaveBeenCalledWith('health_get', {})
    expect(health).toStrictEqual({ connected: true, service_version: '0.1.0' })
  })

  it('wraps a timeline query in the shell request argument', async () => {
    invokeMock.mockResolvedValueOnce({ events: [] })

    await tauriTransport.query('timeline.query', {
      project_id: 'kan',
      since: '2026-09-04T12:00:00Z',
    })

    expect(invokeMock).toHaveBeenCalledWith('timeline_query', {
      request: {
        project_id: 'kan',
        since: '2026-09-04T12:00:00Z',
      },
    })
  })

  it('carries a shell rejection through to the caller', async () => {
    invokeMock.mockRejectedValueOnce({ code: 'internal', message: 'the core is not up' })

    await expect(
      tauriTransport.query('health.get', {}),
    ).rejects.toStrictEqual({ code: 'internal', message: 'the core is not up' })
  })

  it('delivers the shell events as generated envelopes', async () => {
    const handlers = captureListeners()
    const seen: Array<{ sequence: number }> = []
    tauriTransport.subscribe((event) => seen.push(event))

    handlers.get('core://event')?.({
      payload: { sequence: 3, event_type: 'counter.bumped', payload: { to: 3 } },
    })
    expect(seen).toStrictEqual([
      { sequence: 3, event_type: 'counter.bumped', payload: { to: 3 } },
    ])
  })

  it('delivers the shell connection announcements', async () => {
    const handlers = captureListeners()
    const states: string[] = []
    tauriTransport.onConnectionChange((state) => states.push(state))

    handlers.get('core://connection')?.({ payload: { state: 'connected' } })
    handlers.get('core://connection')?.({ payload: { state: 'disconnected' } })
    expect(states).toStrictEqual(['connected', 'disconnected'])
  })

  it('unsubscribes by stopping the listener', async () => {
    const stop = vi.fn()
    listenMock.mockImplementationOnce(() => Promise.resolve(stop))
    const unsubscribe = tauriTransport.subscribe(() => undefined)
    await Promise.resolve()

    unsubscribe()

    expect(stop).toHaveBeenCalled()
  })
})

describe('asApiError', () => {
  it('keeps a shell error that already matches the contract', () => {
    const apiError = { code: 'not_found', message: 'gone' }
    expect(asApiError(apiError)).toBe(apiError)
  })

  it('wraps anything else as an internal error', () => {
    expect(asApiError('the channel closed')).toStrictEqual({
      code: 'internal',
      message: 'the channel closed',
    })
  })
})
