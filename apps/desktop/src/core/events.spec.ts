import { describe, expect, it, vi } from 'vitest'

const { listenMock } = vi.hoisted(() => ({
  listenMock: vi.fn(),
}))
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: listenMock }))

import { tauriTransport } from './transport'

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

describe('live event catalogue', () => {
  it('delivers catalogued shell events as typed live events', async () => {
    const handlers = captureListeners()
    const seen: Array<{ sequence: number; event_type: string }> = []
    tauriTransport.subscribe((event) => seen.push(event))

    handlers.get('core://event')?.({
      payload: {
        sequence: 3,
        event_type: 'comment.created',
        payload: {
          id: 2,
          project_id: 'kan',
          target: { kind: 'ticket', id: 'kan-t11' },
          text: 'Ship it',
          version: 1,
        },
      },
    })

    expect(seen).toStrictEqual([
      {
        sequence: 3,
        event_type: 'comment.created',
        payload: {
          id: 2,
          project_id: 'kan',
          target: { kind: 'ticket', id: 'kan-t11' },
          text: 'Ship it',
          version: 1,
        },
      },
    ])
  })

  it('refuses unknown live event names from the shell', async () => {
    const handlers = captureListeners()
    tauriTransport.subscribe(() => undefined)

    expect(() =>
      handlers.get('core://event')?.({
        payload: { sequence: 1, event_type: 'counter.bumped', payload: { to: 1 } },
      }),
    ).toThrow('unknown live event type `counter.bumped`')
  })
})
