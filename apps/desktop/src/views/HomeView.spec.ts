import { mount, flushPromises } from '@vue/test-utils'
import { createPinia } from 'pinia'
import { describe, expect, it, vi } from 'vitest'
import type { HealthResponse, TimelineQueryResponse } from '@kanban/contracts'
import { kanbanTransportKey } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import HomeView from './HomeView.vue'

// A transport whose health answer and event stream the test steers.
function harness() {
  const queries = vi.fn(async (name: string) => {
    if (name === 'timeline.query') {
      return { events: [] } as TimelineQueryResponse
    }
    return { connected: true, service_version: '0.1.0' } as HealthResponse
  })
  let eventHandler: ((event: { sequence: number }) => void) | undefined
  const transport = {
    query: queries,
    command: () => undefined,
    subscribe(handler: (event: { sequence: number }) => void) {
      eventHandler = handler
      return () => undefined
    },
    onConnectionChange: () => () => undefined,
  } as unknown as ShellTransport
  return {
    transport,
    queries,
    emitEvent: (sequence: number) => eventHandler?.({ sequence }),
  }
}

function mountView(transport: ShellTransport) {
  return mount(HomeView, {
    global: {
      plugins: [createPinia()],
      provide: { [kanbanTransportKey as symbol]: transport },
    },
  })
}

describe('HomeView boot surface', () => {
  it('shows the connecting phase before the client answers', () => {
    const { transport, queries } = harness()
    queries.mockImplementation(() => new Promise<HealthResponse>(() => undefined))

    const view = mountView(transport)

    expect(view.find('[data-testid="connection-status"]').text()).toBe(
      'Connecting to the core…',
    )
  })

  it('shows the core and its version once the client connects', async () => {
    const { transport } = harness()
    const view = mountView(transport)
    await flushPromises()

    expect(view.find('[data-testid="connection-status"]').text()).toBe(
      'Core connected · v0.1.0',
    )
  })

  it('shows an unreachable core when the client cannot verify it', async () => {
    const { transport, queries } = harness()
    queries.mockImplementation(() => Promise.reject(new Error('no core')))
    const view = mountView(transport)
    await flushPromises()

    expect(view.find('[data-testid="connection-status"]').text()).toBe('Core unreachable')
  })

  it('shows the ordered event stream once events arrive', async () => {
    const { transport, emitEvent } = harness()
    const view = mountView(transport)
    await flushPromises()

    expect(view.find('[data-testid="event-stream"]').text()).toBe('Event stream idle')

    emitEvent(7)
    await flushPromises()
    expect(view.find('[data-testid="event-stream"]').text()).toBe(
      'Event stream live · sequence 7',
    )
  })

  it('shows an unselected timeline state once the core connects', async () => {
    const { transport } = harness()
    const view = mountView(transport)
    await flushPromises()

    expect(view.find('[data-testid="timeline-unselected"]').exists()).toBe(true)
    expect(view.find('[data-testid="timeline-surface"]').exists()).toBe(false)
    expect(view.find('[data-testid="timeline-unselected"]').text()).toContain(
      'Select a Project',
    )
  })

  it('does not query the timeline on a clean boot surface', async () => {
    const { transport, queries } = harness()
    mountView(transport)
    await flushPromises()

    const timelineCalls = queries.mock.calls.filter(([name]) => name === 'timeline.query')
    expect(timelineCalls).toHaveLength(0)
  })
})
