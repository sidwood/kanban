import { flushPromises, mount } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type {
  HealthResponse,
  ProjectListResponse,
  ProjectRecord,
  TimelineQueryResponse,
} from '@kanban/contracts'
import router from '../router'
import { kanbanTransportKey } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import HomeView from './HomeView.vue'

function project(overrides: Partial<ProjectRecord> = {}): ProjectRecord {
  return {
    id: 7,
    code: 'CORE',
    name: 'Control plane',
    repository: '/repositories/kanban',
    seed_workspace: '/workspaces/kanban.seed',
    default_branch: 'main',
    herdr_session: 'kanban-main',
    herdr_workspace: 'kanban.seed',
    initiative_id: null,
    archived: false,
    counters: { plan: 0, spec: 0, ticket: 0 },
    version: 1,
    ...overrides,
  }
}

function harness(projects: ProjectRecord[] = [project()]) {
  const query = vi.fn(async (name: string, request?: unknown) => {
    if (name === 'health.get') {
      return { connected: true, service_version: '0.1.0' } satisfies HealthResponse
    }
    if (name === 'project.list') {
      return { projects } satisfies ProjectListResponse
    }
    if (name === 'timeline.query') {
      return { events: [] } satisfies TimelineQueryResponse
    }
    throw new Error(`unexpected query ${name}: ${JSON.stringify(request)}`)
  })
  const transport = {
    query,
    command: vi.fn(),
    subscribe: () => () => undefined,
    onConnectionChange: () => () => undefined,
  } as unknown as ShellTransport
  return { transport, query }
}

async function mounted(projects: ProjectRecord[] = [project()]) {
  const { transport, query } = harness(projects)
  router.push('/')
  await router.isReady()
  const wrapper = mount(HomeView, {
    global: {
      plugins: [createPinia(), router],
      provide: { [kanbanTransportKey as symbol]: transport },
    },
  })
  await flushPromises()
  return { wrapper, transport, query }
}

describe('HomeView timeline routing', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
  })

  it('mounts the timeline for the selected Project numeric identity', async () => {
    const { wrapper, query } = await mounted([
      project({ id: 7 }),
      project({ id: 9, code: 'OPS', name: 'Operations' }),
    ])

    await wrapper.find('[data-testid="home-project-select"]').setValue('9')
    await flushPromises()

    expect(wrapper.find('[data-testid="timeline-unselected"]').exists()).toBe(false)
    expect(wrapper.find('[data-testid="timeline-surface"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="timeline-surface"]').text()).toContain('Project 9')

    const timelineCalls = query.mock.calls.filter(([name]) => name === 'timeline.query')
    expect(timelineCalls).toHaveLength(1)
    expect(timelineCalls[0]?.[1]).toStrictEqual({
      scope: { project: 9 },
      entity: undefined,
      kinds: undefined,
      since: undefined,
      until: undefined,
    })
  })
})
