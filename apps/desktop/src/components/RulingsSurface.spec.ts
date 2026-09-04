import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import type {
  DeferralListResponse,
  KanbanTransport,
  RulingListResponse,
} from '@kanban/contracts'
import RulingsSurface from '../components/RulingsSurface.vue'
import { kanbanTransportKey } from '../core/transport'

function transportWithRecords(
  rulings: RulingListResponse,
  deferrals: DeferralListResponse,
): KanbanTransport & { onConnectionChange: () => () => void } {
  return {
    async query(name, request) {
      if (name === 'ruling.list') {
        return rulings as never
      }
      if (name === 'deferral.list') {
        return deferrals as never
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

describe('rulings surface', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
  })

  it('renders rulings and deferrals with supersession provenance', async () => {
    const transport = transportWithRecords(
      {
        rulings: [
          {
            id: 1,
            project_id: 'kan',
            summary: 'Hold',
            recorded_at: '2026-09-04T12:00:01Z',
          },
          {
            id: 2,
            project_id: 'kan',
            summary: 'Proceed',
            supersedes_id: 1,
            recorded_at: '2026-09-04T12:00:02Z',
          },
        ],
      },
      {
        deferrals: [
          {
            id: 1,
            project_id: 'kan',
            finding_id: 'finding-1',
            reason: 'Cosmetic only',
            recorded_at: '2026-09-04T12:00:03Z',
          },
          {
            id: 2,
            project_id: 'kan',
            finding_id: 'finding-1',
            reason: 'Accepted risk',
            supersedes_id: 1,
            recorded_at: '2026-09-04T12:00:04Z',
          },
        ],
      },
    )

    const wrapper = mount(RulingsSurface, {
      props: {
        projectId: 'kan',
        entityKind: 'ticket',
        entityId: 'kan-t12',
      },
      global: {
        provide: {
          [kanbanTransportKey as symbol]: transport,
        },
      },
    })

    await flushPromises()

    expect(wrapper.get('[data-testid="ruling-1"]').text()).toContain('Hold')
    expect(wrapper.get('[data-testid="ruling-2"]').text()).toContain('Proceed')
    expect(wrapper.get('[data-testid="ruling-supersedes-2"]').text()).toContain('Supersedes ruling 1')
    expect(wrapper.get('[data-testid="deferral-1"]').text()).toContain('Cosmetic only')
    expect(wrapper.get('[data-testid="deferral-2"]').text()).toContain('Accepted risk')
    expect(wrapper.get('[data-testid="deferral-supersedes-2"]').text()).toContain('Supersedes deferral 1')
  })

  it('loads rulings through the generated list queries', async () => {
    const query = vi.fn(async (name: string) => {
      if (name === 'ruling.list') {
        return { rulings: [] }
      }
      if (name === 'deferral.list') {
        return { deferrals: [] }
      }
      throw new Error(`unexpected query ${name}`)
    })
    const transport = {
      query,
      command: vi.fn(),
      subscribe: () => () => undefined,
      onConnectionChange: () => () => undefined,
    }

    mount(RulingsSurface, {
      props: { projectId: 'kan', entityKind: 'ticket', entityId: 'kan-t12' },
      global: {
        provide: {
          [kanbanTransportKey as symbol]: transport,
        },
      },
    })
    await flushPromises()

    expect(query).toHaveBeenCalledWith('ruling.list', {
      project_id: 'kan',
      entity: { kind: 'ticket', id: 'kan-t12' },
    })
    expect(query).toHaveBeenCalledWith('deferral.list', {
      project_id: 'kan',
    })
  })
})
