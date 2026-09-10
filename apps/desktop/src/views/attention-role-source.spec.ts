// A produced Role Attention source has an action (KAN-T140-AC2,
// KAN-T140-AC6, KAN-S11-US4). Production emits a Role subject for an
// observed role that stalled or settled without its required result,
// and the only record this application holds of that role is its
// Herdr telemetry on the Project's activity timeline — so that is
// where the item's action leads, marked to the exact role. Opening it
// stays a read: nothing is acknowledged by navigating.
import { flushPromises, mount } from '@vue/test-utils'
import type { VueWrapper } from '@vue/test-utils'
import { createPinia } from 'pinia'
import { afterEach, describe, expect, it } from 'vitest'
import type { AttentionItemRecord, ProjectRecord } from '@kanban/contracts'
import router from '../router'
import { kanbanTransportKey } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import { destinationForAttentionItem } from '../stores/attention-navigation'
import AttentionInboxView from './AttentionInboxView.vue'
import HomeView from './HomeView.vue'

const projects: ProjectRecord[] = [
  {
    id: 3,
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
]

// Exactly the item `runtime_attention` emits for a settled role that
// never supplied its required result.
const roleItem: AttentionItemRecord = {
  id: 'role:reviewer',
  project_id: 3,
  kind: 'missing_result',
  subject_kind: 'role',
  subject_id: 'reviewer',
  summary: 'A settled role has not supplied its required result.',
  detail: {
    source: 'missing_result_deadline_breached',
    role: 'reviewer',
    settled_unix_secs: 1789000000,
  },
  active: true,
  acknowledged_at: null,
  acknowledged_by: null,
  first_seen_at: '2026-09-14T06:00:00Z',
  last_seen_at: '2026-09-14T06:05:00Z',
  version: 1,
}

const events = [
  {
    id: 1,
    scope: { project: 3 },
    kind: 'telemetry',
    entity: null,
    recorded_at: '2026-09-14T06:00:00Z',
    detail: { source: 'herdr', event: 'role.settled', role: 'reviewer', payload: {} },
  },
  {
    id: 2,
    scope: { project: 3 },
    kind: 'telemetry',
    entity: null,
    recorded_at: '2026-09-14T06:01:00Z',
    detail: { source: 'herdr', event: 'role.output', role: 'implementer', payload: {} },
  },
]

function harness() {
  const commands: Array<{ name: string; request: unknown }> = []
  const queries: Array<{ name: string; request: Record<string, unknown> }> = []
  const transport = {
    query: (name: string, request: Record<string, unknown>) => {
      queries.push({ name, request })
      switch (name) {
        case 'attention.list':
          return Promise.resolve({ items: [roleItem] })
        case 'project.list':
          return Promise.resolve({ projects })
        case 'timeline.query':
          return Promise.resolve({ events })
        case 'ruling.list':
          return Promise.resolve({ rulings: [] })
        case 'deferral.list':
          return Promise.resolve({ deferrals: [] })
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
        case 'notification.settings.get':
          return Promise.resolve({ settings: { enabled: false, kinds: [] } })
        default:
          return Promise.resolve({})
      }
    },
    command: (name: string, request: unknown) => {
      commands.push({ name, request })
      return Promise.resolve({})
    },
    subscribe: () => () => undefined,
    onConnectionChange: () => () => undefined,
  } as unknown as ShellTransport
  return { transport, commands, queries }
}

const mounted: VueWrapper[] = []
afterEach(() => {
  for (const wrapper of mounted.splice(0)) wrapper.unmount()
})

async function mountAt(component: unknown, path: string, transport: ShellTransport) {
  await router.push(path)
  await router.isReady()
  const wrapper = mount(component as never, {
    global: {
      plugins: [createPinia(), router],
      provide: { [kanbanTransportKey as symbol]: transport },
    },
  })
  mounted.push(wrapper)
  await flushPromises()
  return wrapper
}

describe('Role attention source', () => {
  it('opens the role’s own activity in its own Project', () => {
    const destination = destinationForAttentionItem(roleItem)

    expect(destination.route).toBe('/activity?project=3&role=reviewer')
    expect(destination.label).toContain('role')
  })

  it('escapes a role name that is not a bare path segment', () => {
    expect(
      destinationForAttentionItem({ ...roleItem, subject_id: 'reviewer two' }).route,
    ).toBe('/activity?project=3&role=reviewer%20two')
    expect(destinationForAttentionItem({ ...roleItem, subject_id: 'a/b' }).route).toBe(
      '/activity?project=3&role=a%2Fb',
    )
  })

  it('offers the action on the produced item rather than stating no surface', async () => {
    const { transport, commands } = harness()
    const wrapper = await mountAt(AttentionInboxView, '/attention', transport)

    const open = wrapper.get('[data-testid="attention-open"]')
    expect(open.attributes('href')).toBe('/activity?project=3&role=reviewer')
    expect(wrapper.find('[data-testid="attention-no-surface"]').exists()).toBe(false)
    expect(commands).toEqual([])
  })

  it('marks the named role’s own activity on the receiving surface', async () => {
    const { transport, commands } = harness()
    const wrapper = await mountAt(HomeView, '/activity?project=3&role=reviewer', transport)

    expect(wrapper.get('[data-testid="activity-role"]').text()).toContain('reviewer')
    expect(wrapper.get('[data-testid="timeline-event-1"]').attributes('data-linked')).toBe('true')
    expect(wrapper.get('[data-testid="timeline-event-2"]').attributes('data-linked')).toBeUndefined()
    expect(commands).toEqual([])
  })

  it('reads the role’s telemetry, not the whole Project history', async () => {
    const { transport, queries } = harness()
    await mountAt(HomeView, '/activity?project=3&role=reviewer', transport)

    const timeline = queries.filter((entry) => entry.name === 'timeline.query')
    expect(timeline.length).toBeGreaterThan(0)
    expect(timeline.at(-1)!.request.kinds).toEqual(['telemetry'])
  })

  it('leaves the timeline unmarked and unfiltered when no role is named', async () => {
    const { transport, queries } = harness()
    const wrapper = await mountAt(HomeView, '/activity?project=3', transport)

    expect(wrapper.find('[data-testid="activity-role"]').exists()).toBe(false)
    expect(wrapper.get('[data-testid="timeline-event-1"]').attributes('data-linked')).toBeUndefined()
    const timeline = queries.filter((entry) => entry.name === 'timeline.query')
    expect(timeline.at(-1)!.request.kinds).toBeUndefined()
  })
})
