// The receiving half of exact object context (KAN-T140-AC2,
// KAN-T140-AC6): a link that names an Initiative or a Project's
// activity opens that object, rather than leaving the operator on the
// list to find it again.
import { flushPromises, mount } from '@vue/test-utils'
import type { VueWrapper } from '@vue/test-utils'
import { createPinia } from 'pinia'
import { afterEach, describe, expect, it } from 'vitest'
import type { InitiativeRecord, ProjectRecord } from '@kanban/contracts'
import router from '../router'
import { kanbanTransportKey } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import HomeView from './HomeView.vue'
import InitiativesView from './InitiativesView.vue'

const initiatives: InitiativeRecord[] = [
  { id: 1, name: 'Personal tooling', archived: false, version: 1 },
  { id: 2, name: 'Fleet recovery', archived: false, version: 1 },
]

const projects: ProjectRecord[] = [
  {
    id: 1,
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
  },
  {
    id: 4,
    code: 'EDGE',
    name: 'Edge tooling',
    repository: '/repositories/edge',
    seed_workspace: '/workspaces/edge.seed',
    default_branch: 'main',
    herdr_session: null,
    herdr_workspace: 'edge.seed',
    initiative_id: null,
    archived: false,
    counters: { plan: 0, spec: 0, ticket: 0 },
    version: 1,
  },
]

function transport(): ShellTransport {
  return {
    query: (name: string) => {
      switch (name) {
        case 'initiative.list':
          return Promise.resolve({ initiatives })
        case 'project.list':
          return Promise.resolve({ projects })
        case 'health.get':
          return Promise.resolve({
            connected: true,
            service_version: '0.1.0',
            service: { started_at: '2026-09-14T06:00:00Z' },
            database: { journal_mode: 'wal', last_change_at: null, schema_version: 1 },
            scheduler: { last_backup_success_at: null },
            mcp: { exposed_tools: 0 },
            herdr: { connection_diagnostic: null, sessions: [] },
            workspaces: {
              by_health: { assigned: 0, available: 0, dirty: 0, missing: 0, retired: 0, unobserved: 0 },
              last_change_at: null,
            },
          })
        case 'timeline.query':
          return Promise.resolve({ events: [] })
        case 'ruling.list':
          return Promise.resolve({ rulings: [] })
        case 'deferral.list':
          return Promise.resolve({ deferrals: [] })
        default:
          return Promise.resolve({})
      }
    },
    command: () => Promise.resolve({}),
    subscribe: () => () => undefined,
    onConnectionChange: () => () => undefined,
  } as unknown as ShellTransport
}

const mounted: VueWrapper[] = []
afterEach(() => {
  for (const wrapper of mounted.splice(0)) wrapper.unmount()
})

async function mountAt(component: unknown, path: string) {
  await router.push(path)
  await router.isReady()
  const wrapper = mount(component as never, {
    global: {
      plugins: [createPinia(), router],
      provide: { [kanbanTransportKey as symbol]: transport() },
    },
  })
  mounted.push(wrapper)
  await flushPromises()
  return wrapper
}

describe('linked objects', () => {
  it('marks the exact Initiative a link names', async () => {
    const wrapper = await mountAt(InitiativesView, '/initiatives?initiative=2')

    expect(wrapper.get('[data-testid="initiative-row-2"]').attributes('data-linked')).toBe('true')
    expect(wrapper.get('[data-testid="initiative-row-1"]').attributes('data-linked')).toBeUndefined()
  })

  it('leaves every row unmarked when the link names none', async () => {
    const wrapper = await mountAt(InitiativesView, '/initiatives')

    expect(wrapper.get('[data-testid="initiative-row-2"]').attributes('data-linked')).toBeUndefined()
  })

  it('opens the Project activity a link names', async () => {
    const wrapper = await mountAt(HomeView, '/activity?project=4&deferral=2')

    expect((wrapper.get('[data-testid="home-project-select"]').element as HTMLSelectElement).value).toBe('4')
  })
})
