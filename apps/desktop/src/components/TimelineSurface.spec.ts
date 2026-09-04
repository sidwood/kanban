import { createPinia, setActivePinia } from 'pinia'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import type { KanbanTransport, TimelineQueryResponse } from '@kanban/contracts'
import TimelineSurface from '../components/TimelineSurface.vue'
import { useTimelineStore } from '../stores/timeline'
import { kanbanTransportKey } from '../core/transport'

function transportWithTimeline(
  response: TimelineQueryResponse,
): KanbanTransport & { onConnectionChange: () => () => void } {
  return {
    async query(name, request) {
      if (name === 'timeline.query') {
        return response as never
      }
      throw new Error(`unexpected query ${name}: ${JSON.stringify(request)}`)
    },
    async command() {
      throw new Error('unexpected command')
    },
    subscribe: () => () => undefined,
    onConnectionChange: () => () => undefined,
  }
}

describe('timeline surface', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.stubEnv('TZ', 'UTC')
  })

  afterEach(() => {
    vi.unstubAllEnvs()
  })

  it('renders events from the generated timeline query', async () => {
    const transport = transportWithTimeline({
      events: [
        {
          id: 1,
          scope: { project: 'kan' },
          kind: 'transition',
          entity: { kind: 'ticket', id: 'kan-t9' },
          recorded_at: '2026-09-04T12:00:01Z',
          detail: { to: 'in_progress' },
        },
      ],
    })

    const wrapper = mount(TimelineSurface, {
      props: {
        scope: { project: 'kan' },
        entityKind: 'ticket',
        entityId: 'kan-t9',
      },
      global: {
        provide: {
          [kanbanTransportKey as symbol]: transport,
        },
      },
    })

    await vi.waitFor(() => {
      expect(wrapper.get('[data-testid="timeline-event-1"]').text()).toContain('transition')
    })
    expect(wrapper.get('[data-testid="timeline-event-1"]').text()).toContain('kan-t9')
  })

  it('loads the timeline when real project and entity selection are provided', async () => {
    const query = vi.fn(async () => ({ events: [] }))
    const transport = {
      query,
      command: vi.fn(),
      subscribe: () => () => undefined,
      onConnectionChange: () => () => undefined,
    }

    mount(TimelineSurface, {
      props: {
        scope: { project: 'my-project' },
        entityKind: 'ticket',
        entityId: 'my-ticket',
      },
      global: {
        provide: {
          [kanbanTransportKey as symbol]: transport,
        },
      },
    })
    await flushPromises()

    expect(query).toHaveBeenCalledWith('timeline.query', {
      scope: { project: 'my-project' },
      entity: { kind: 'ticket', id: 'my-ticket' },
      kinds: undefined,
      since: undefined,
      until: undefined,
    })
  })

  it('applies entity, kind, and time filters through the store', async () => {
    const query = vi.fn(async () => ({
      events: [],
    }))
    const transport = {
      query,
      command: vi.fn(),
      subscribe: () => () => undefined,
      onConnectionChange: () => () => undefined,
    }

    const wrapper = mount(TimelineSurface, {
      props: { scope: { project: 'kan' } },
      global: {
        provide: {
          [kanbanTransportKey as symbol]: transport,
        },
      },
    })
    await flushPromises()
    query.mockClear()

    const store = useTimelineStore()
    store.setEntityFilter('ticket', 'kan-t9')
    store.setKindFilter(['transition'])
    store.setSince('2026-09-04T00:00')
    store.setUntil('2026-09-04T23:59')

    await wrapper.find('form').trigger('submit.prevent')
    await flushPromises()

    expect(query).toHaveBeenCalledWith('timeline.query', {
      scope: { project: 'kan' },
      entity: { kind: 'ticket', id: 'kan-t9' },
      kinds: ['transition'],
      since: '2026-09-04T00:00:00.000Z',
      until: '2026-09-04T23:59:59.999Z',
    })
  })
})
