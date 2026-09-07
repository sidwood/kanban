import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import type { HealthResponse, KanbanTransport } from '@kanban/contracts'
import HealthDashboard from './HealthDashboard.vue'
import { kanbanTransportKey } from '../core/transport'
import { useHealthStore } from '../stores/health'

// A full health answer: every component carrying detail, and the
// last-change times each component already records.
function healthAnswer(overrides: Partial<HealthResponse> = {}): HealthResponse {
  return {
    connected: true,
    service_version: '0.1.0',
    service: { started_at: '2026-09-07T09:00:00Z' },
    database: {
      journal_mode: 'wal',
      schema_version: 31,
      last_change_at: '2026-09-07T10:15:30.500Z',
    },
    scheduler: { last_backup_success_at: '2026-09-07T04:00:00Z' },
    mcp: { exposed_tools: 42 },
    herdr: {
      sessions: [
        {
          project_id: 1,
          diagnostics: {
            session_name: 'kanban-main',
            product_workspace: '/workspaces/kanban.seed',
            herdr_workspace: 'kanban.seed',
            connected: true,
            last_snapshot_at: '2026-09-07T10:00:00Z',
            last_error: null,
          },
        },
        {
          project_id: 2,
          diagnostics: {
            session_name: null,
            product_workspace: '/workspaces/wave.seed',
            herdr_workspace: 'wave.seed',
            connected: false,
            last_snapshot_at: null,
            last_error: 'the session socket is absent',
          },
        },
      ],
    },
    workspaces: {
      by_health: {
        available: 2,
        assigned: 1,
        dirty: 1,
        missing: 3,
        retired: 1,
        unobserved: 1,
      },
      last_change_at: '2026-09-07T09:30:00Z',
    },
    ...overrides,
  }
}

function harness(answer: HealthResponse = healthAnswer()) {
  const query = vi.fn(async (name: string, request?: unknown) => {
    if (name === 'health.get') {
      return answer
    }
    throw new Error(`unexpected query ${name}: ${JSON.stringify(request)}`)
  })
  const transport = {
    query,
    command: async () => {
      throw new Error('no commands are catalogued')
    },
    subscribe: () => () => undefined,
    onConnectionChange: () => () => undefined,
  } as unknown as KanbanTransport & { onConnectionChange: () => () => void }
  return { transport, query }
}

async function mounted(transport: unknown) {
  const wrapper = mount(HealthDashboard, {
    global: {
      plugins: [createPinia()],
      provide: { [kanbanTransportKey as symbol]: transport },
    },
  })
  await flushPromises()
  return wrapper
}

describe('health dashboard', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
  })

  it('renders the query once for every component', async () => {
    const { transport, query } = harness()

    const wrapper = await mounted(transport)

    expect(query).toHaveBeenCalledWith('health.get', {})
    expect(wrapper.find('[data-testid="health-service-connected"]').text()).toBe('yes')
    expect(wrapper.find('[data-testid="health-service-version"]').text()).toBe('0.1.0')
  })

  it('renders per-component detail with the last-change times', async () => {
    const { transport } = harness()

    const wrapper = await mounted(transport)

    // Service: its state changed by coming into being.
    expect(wrapper.find('[data-testid="health-service-started"]').text())
      .toBe('2026-09-07T09:00:00Z')
    // Database.
    expect(wrapper.find('[data-testid="health-database-journal"]').text()).toBe('wal')
    expect(wrapper.find('[data-testid="health-database-schema"]').text()).toBe('31')
    expect(wrapper.find('[data-testid="health-database-last-change"]').text())
      .toBe('2026-09-07T10:15:30.500Z')
    // Scheduler.
    expect(wrapper.find('[data-testid="health-scheduler-last-backup"]').text())
      .toBe('2026-09-07T04:00:00Z')
    // MCP.
    expect(wrapper.find('[data-testid="health-mcp-tools"]').text()).toBe('42')
    // Workspaces: the census and its last change.
    expect(wrapper.find('[data-testid="health-workspace-available"]').text()).toBe('2')
    expect(wrapper.find('[data-testid="health-workspace-assigned"]').text()).toBe('1')
    expect(wrapper.find('[data-testid="health-workspace-dirty"]').text()).toBe('1')
    expect(wrapper.find('[data-testid="health-workspace-missing"]').text()).toBe('3')
    expect(wrapper.find('[data-testid="health-workspace-retired"]').text()).toBe('1')
    expect(wrapper.find('[data-testid="health-workspace-unobserved"]').text()).toBe('1')
    expect(wrapper.find('[data-testid="health-workspaces-last-change"]').text())
      .toBe('2026-09-07T09:30:00Z')
  })

  it('renders one row per Herdr session with its own detail', async () => {
    const { transport } = harness()

    const wrapper = await mounted(transport)

    const rows = wrapper.findAll('[data-testid="health-herdr-session"]')
    expect(rows).toHaveLength(2)
    expect(rows[0].find('[data-testid="health-herdr-project"]').text()).toBe('1')
    expect(rows[0].find('[data-testid="health-herdr-name"]').text()).toBe('kanban-main')
    expect(rows[0].find('[data-testid="health-herdr-connected"]').text()).toBe('yes')
    expect(rows[0].find('[data-testid="health-herdr-snapshot"]').text())
      .toBe('2026-09-07T10:00:00Z')
    expect(rows[0].find('[data-testid="health-herdr-error"]').text()).toBe('none')
    expect(rows[1].find('[data-testid="health-herdr-name"]').text()).toBe('default session')
    expect(rows[1].find('[data-testid="health-herdr-connected"]').text()).toBe('no')
    expect(rows[1].find('[data-testid="health-herdr-snapshot"]').text()).toBe('never')
    expect(rows[1].find('[data-testid="health-herdr-error"]').text())
      .toBe('the session socket is absent')
  })

  it('renders the markers when no time is recorded yet', async () => {
    const fresh = healthAnswer({
      database: { journal_mode: 'wal', schema_version: 31 },
      scheduler: {},
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
    })
    const { transport } = harness(fresh)

    const wrapper = await mounted(transport)

    expect(wrapper.find('[data-testid="health-database-last-change"]').text()).toBe('never')
    expect(wrapper.find('[data-testid="health-scheduler-last-backup"]').text()).toBe('never')
    expect(wrapper.find('[data-testid="health-workspaces-last-change"]').text()).toBe('never')
  })

  it('reports the query refusal and rechecks through the client', async () => {
    const query = vi.fn(async (): Promise<HealthResponse> => {
      // The shell rejects with the generated ApiError shape.
      throw { code: 'internal', message: 'the core is unreachable' }
    })
    const transport = {
      query,
      command: async () => {
        throw new Error('no commands are catalogued')
      },
      subscribe: () => () => undefined,
      onConnectionChange: () => () => undefined,
    } as unknown as Parameters<typeof mounted>[0]

    const wrapper = await mounted(transport)

    expect(wrapper.find('[data-testid="health-error"]').text())
      .toBe('the core is unreachable')
    expect(wrapper.find('[data-testid="health-service-connected"]').exists()).toBe(false)

    query.mockImplementation(async () => healthAnswer())
    await wrapper.find('[data-testid="health-recheck"]').trigger('click')
    await flushPromises()

    expect(query).toHaveBeenCalledTimes(2)
    expect(wrapper.find('[data-testid="health-service-connected"]').text()).toBe('yes')
    expect(wrapper.find('[data-testid="health-error"]').exists()).toBe(false)
    expect(useHealthStore().health?.service_version).toBe('0.1.0')
  })
})
