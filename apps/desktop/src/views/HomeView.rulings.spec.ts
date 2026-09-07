import { flushPromises, mount } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type {
  DeferralListResponse,
  HealthResponse,
  ProjectListResponse,
  ProjectRecord,
  RulingListResponse,
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
  const query = vi.fn(async (name: string) => {
    if (name === 'health.get') {
      return {
        connected: true,
        service_version: '0.1.0',
        service: { started_at: '2026-09-07T09:00:00Z' },
        database: { journal_mode: 'wal', schema_version: 1 },
        scheduler: {},
        mcp: { exposed_tools: 1 },
        herdr: { sessions: [] },
        workspaces: {
          by_health: {
            available: 0,
            assigned: 0,
            dirty: 0,
            missing: 0,
            retired: 0,
            unobserved: 0,
          },
        },
      } satisfies HealthResponse
    }
    if (name === 'project.list') {
      return { projects } satisfies ProjectListResponse
    }
    if (name === 'ruling.list') {
      return { rulings: [] } satisfies RulingListResponse
    }
    if (name === 'deferral.list') {
      return { deferrals: [] } satisfies DeferralListResponse
    }
    throw new Error(`unexpected query ${name}`)
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

describe('HomeView rulings routing', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
  })

  it('mounts rulings for the selected Project without fixture entity identities', async () => {
    const { wrapper, query } = await mounted([project({ id: 7 })])

    await wrapper.find('[data-testid="home-project-select"]').setValue('7')
    await flushPromises()

    expect(wrapper.find('[data-testid="rulings-surface"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="rulings-surface"]').text()).toContain('Project 7')

    expect(query).toHaveBeenCalledWith('ruling.list', { project_id: 7 })
    expect(query).toHaveBeenCalledWith('deferral.list', { project_id: 7 })
  })
})
