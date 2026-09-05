import { mount, flushPromises } from '@vue/test-utils'
import { createPinia } from 'pinia'
import { describe, expect, it, vi } from 'vitest'
import type {
  HerdrDefaultsGetResponse,
  HerdrSettingsGetResponse,
  ProjectListResponse,
  ProjectRecord,
} from '@kanban/contracts'
import router from '../router'
import { kanbanTransportKey } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import HerdrSettingsView from '../views/HerdrSettingsView.vue'
import { useHerdrSettingsStore } from './herdr-settings'

function project(overrides: Partial<ProjectRecord> = {}): ProjectRecord {
  return {
    id: 1,
    code: 'CORE',
    name: 'Control plane',
    repository: '/repositories/kanban',
    seed_workspace: '/workspaces/kanban.seed',
    default_branch: 'main',
    herdr_session: 'kanban-main',
    initiative_id: null,
    archived: false,
    counters: { plan: 0, spec: 0, ticket: 0 },
    version: 1,
    ...overrides,
  }
}

function settingsResponse(overrides: Partial<HerdrSettingsGetResponse> = {}): HerdrSettingsGetResponse {
  return {
    project_id: 1,
    settings: {
      reconciliation_interval_secs: 300,
      polling_fallback_enabled: false,
      polling_fallback_interval_secs: 10,
      stall_deadline_secs: 3600,
      missing_result_deadline_secs: 7200,
      version: 1,
    },
    diagnostics: {
      session_name: 'kanban-main',
      product_workspace: '/workspaces/kanban.seed',
      herdr_workspace: 'kanban.seed',
      connected: true,
      last_snapshot_at: '2026-09-05T04:46:00Z',
      last_error: null,
    },
    ...overrides,
  }
}

function harness() {
  const command = vi.fn(() =>
    Promise.resolve({
      reconciliation_interval_secs: 600,
      polling_fallback_enabled: true,
      polling_fallback_interval_secs: 10,
      stall_deadline_secs: 1800,
      missing_result_deadline_secs: 3600,
      version: 2,
    }),
  )
  let currentSettings = settingsResponse()
  const queryWithUpdates = vi.fn((name: string) => {
    if (name === 'project.list') {
      return Promise.resolve({ projects: [project()] } satisfies ProjectListResponse)
    }
    if (name === 'herdr.defaults.get') {
      return Promise.resolve({
        defaults: {
          reconciliation_interval_secs: 300,
          stall_deadline_secs: 3600,
          missing_result_deadline_secs: 7200,
          version: 1,
        },
      } satisfies HerdrDefaultsGetResponse)
    }
    currentSettings = {
      ...currentSettings,
      settings: {
        reconciliation_interval_secs: 600,
        polling_fallback_enabled: true,
        polling_fallback_interval_secs: 10,
        stall_deadline_secs: 1800,
        missing_result_deadline_secs: 3600,
        version: 2,
      },
    }
    return Promise.resolve(currentSettings)
  })
  const transport = {
    query: queryWithUpdates,
    command,
    subscribe: () => () => undefined,
    onConnectionChange: () => () => undefined,
  } as unknown as ShellTransport
  return { transport, query: queryWithUpdates, command }
}

async function mounted() {
  const { transport, query, command } = harness()
  router.push('/settings/herdr')
  await router.isReady()
  const wrapper = mount(HerdrSettingsView, {
    global: {
      plugins: [createPinia(), router],
      provide: { [kanbanTransportKey as symbol]: transport },
    },
  })
  await flushPromises()
  return { wrapper, transport, query, command, store: useHerdrSettingsStore() }
}

describe('herdr-settings', () => {
  it('loads global defaults and project settings with diagnostics', async () => {
    const { wrapper, query } = await mounted()

    expect(query).toHaveBeenCalledWith('project.list', {})
    expect(query).toHaveBeenCalledWith('herdr.defaults.get', {})
    expect(query).toHaveBeenCalledWith('herdr.settings.get', { project_id: 1 })
    expect(wrapper.find('[data-testid="defaults-reconciliation"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="settings-reconciliation"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="diagnostics-connected"]').text()).toBe('yes')
    expect(wrapper.find('[data-testid="diagnostics-session"]').text()).toBe('kanban-main')
  })

  it('saves updated project settings through the generated client', async () => {
    const { wrapper, command, store } = await mounted()
    store.settings = {
      reconciliation_interval_secs: 600,
      polling_fallback_enabled: true,
      polling_fallback_interval_secs: 10,
      stall_deadline_secs: 1800,
      missing_result_deadline_secs: 3600,
      version: 1,
    }

    await wrapper.find('[data-testid="save-project-settings"]').trigger('click')
    await flushPromises()

    expect(command).toHaveBeenCalledWith(
      'herdr.settings.update',
      expect.objectContaining({
        project_id: 1,
        reconciliation_interval_secs: 600,
        polling_fallback_enabled: true,
      }),
    )
    expect(store.settings?.version).toBe(2)
  })

  it('shows reconciliation, fallback polling, deadlines, and diagnostics fields', async () => {
    const { wrapper } = await mounted()

    expect(wrapper.find('[data-testid="settings-polling-enabled"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="settings-polling-interval"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="settings-stall"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="settings-missing-result"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="diagnostics-product-workspace"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="diagnostics-last-snapshot"]').exists()).toBe(true)
  })
})
