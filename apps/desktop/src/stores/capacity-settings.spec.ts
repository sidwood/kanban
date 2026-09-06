import { mount, flushPromises } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import { describe, expect, it, vi } from 'vitest'
import type {
  CapacityDefaultsGetResponse,
  CapacityProjectCaps,
  CapacitySettingsGetResponse,
  ProjectListResponse,
  ProjectRecord,
} from '@kanban/contracts'
import router from '../router'
import { kanbanTransportKey } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import CapacitySettingsView from '../views/CapacitySettingsView.vue'
import { useCapacityStore } from './capacity-settings'

function project(overrides: Partial<ProjectRecord> = {}): ProjectRecord {
  return {
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
    ...overrides,
  }
}

function unsetCaps(): CapacityProjectCaps {
  return {
    max_active_per_harness: null,
    max_active_per_model: null,
    max_active_per_usage_pool: null,
    max_active_lanes: null,
    version: 1,
  }
}

function capsResponse(caps: CapacityProjectCaps): CapacitySettingsGetResponse {
  return { project_id: 1, caps }
}

const defaults: CapacityDefaultsGetResponse = {
  defaults: {
    max_active_per_harness: 2,
    max_active_per_model: 2,
    max_active_per_usage_pool: 4,
    version: 1,
  },
}

function harness(storedCaps = unsetCaps()) {
  // The caps the query answers with, replaced by the commands so a
  // reload reflects what landed.
  let current = storedCaps
  const command = vi.fn((name: string, request: unknown) => {
    const asked = request as Record<string, unknown>
    if (name === 'capacity.defaults.update') {
      return Promise.resolve({
        max_active_per_harness: asked.max_active_per_harness,
        max_active_per_model: asked.max_active_per_model,
        max_active_per_usage_pool: asked.max_active_per_usage_pool,
        version: 2,
      })
    }
    current = {
      max_active_per_harness: (asked.max_active_per_harness as number | undefined) ?? null,
      max_active_per_model: (asked.max_active_per_model as number | undefined) ?? null,
      max_active_per_usage_pool:
        (asked.max_active_per_usage_pool as number | undefined) ?? null,
      max_active_lanes: (asked.max_active_lanes as number | undefined) ?? null,
      version: current.version + 1,
    }
    return Promise.resolve(current)
  })
  const query = vi.fn((name: string) => {
    if (name === 'project.list') {
      return Promise.resolve({ projects: [project()] } satisfies ProjectListResponse)
    }
    if (name === 'capacity.defaults.get') {
      return Promise.resolve(defaults)
    }
    if (name === 'capacity.settings.get') {
      return Promise.resolve(capsResponse(current))
    }
    return Promise.reject(new Error(`unexpected query ${name}`))
  })
  const transport = {
    query,
    command,
    subscribe: () => () => undefined,
    onConnectionChange: () => () => undefined,
  } as unknown as ShellTransport
  return { transport, query, command, readCaps: () => current }
}

async function mounted(storedCaps = unsetCaps()) {
  const pieces = harness(storedCaps)
  router.push('/settings/capacity')
  await router.isReady()
  const wrapper = mount(CapacitySettingsView, {
    global: {
      plugins: [createPinia(), router],
      provide: { [kanbanTransportKey as symbol]: pieces.transport },
    },
  })
  await flushPromises()
  return { ...pieces, wrapper, store: useCapacityStore() }
}

describe('capacity-settings', () => {
  it('loads the global defaults and the project caps through the generated client', async () => {
    const { wrapper, query } = await mounted()

    expect(query).toHaveBeenCalledWith('project.list', {})
    expect(query).toHaveBeenCalledWith('capacity.defaults.get', {})
    expect(query).toHaveBeenCalledWith('capacity.settings.get', { project_id: 1 })
    expect(
      (wrapper.find('[data-testid="defaults-harness"]').element as HTMLInputElement).value,
    ).toBe('2')
    expect(
      (wrapper.find('[data-testid="defaults-usage-pool"]').element as HTMLInputElement).value,
    ).toBe('4')
  })

  it('shows unset caps as empty fields that impose nothing', async () => {
    const { wrapper } = await mounted()

    for (const field of ['caps-harness', 'caps-model', 'caps-usage-pool', 'caps-lanes']) {
      expect(
        (wrapper.find(`[data-testid="${field}"]`).element as HTMLInputElement).value,
      ).toBe('')
    }
  })

  it('saves the global defaults with their current version', async () => {
    const { wrapper, command, store } = await mounted()
    await wrapper.find('[data-testid="defaults-model"]').setValue('3')
    await wrapper.find('[data-testid="save-defaults"]').trigger('click')
    await flushPromises()

    expect(command).toHaveBeenCalledWith(
      'capacity.defaults.update',
      expect.objectContaining({
        max_active_per_harness: 2,
        max_active_per_model: 3,
        max_active_per_usage_pool: 4,
        mutation: expect.objectContaining({ optimistic_version: 1 }),
      }),
    )
    expect(store.defaults?.version).toBe(2)
  })

  it('sends only the stricter caps that carry a value', async () => {
    const { wrapper, command } = await mounted()

    await wrapper.find('[data-testid="caps-harness"]').setValue('1')
    await wrapper.find('[data-testid="caps-lanes"]').setValue('3')
    await wrapper.find('[data-testid="save-project-caps"]').trigger('click')
    await flushPromises()

    expect(command).toHaveBeenCalledWith(
      'capacity.settings.update',
      expect.objectContaining({
        project_id: 1,
        max_active_per_harness: 1,
        max_active_lanes: 3,
        mutation: expect.objectContaining({ optimistic_version: 1 }),
      }),
    )
    const sent = command.mock.calls[0]?.[1] as Record<string, unknown>
    expect(sent.max_active_model).toBeUndefined()
    expect(sent.max_active_per_usage_pool).toBeUndefined()
  })

  it('renders stored caps into their fields', async () => {
    const stored = unsetCaps()
    stored.max_active_per_harness = 2
    stored.max_active_lanes = 3
    stored.version = 4
    const { wrapper } = await mounted(stored)

    expect(
      (wrapper.find('[data-testid="caps-harness"]').element as HTMLInputElement).value,
    ).toBe('2')
    expect(
      (wrapper.find('[data-testid="caps-lanes"]').element as HTMLInputElement).value,
    ).toBe('3')
  })

  it('clears a stored cap by leaving its field empty', async () => {
    const stored = unsetCaps()
    stored.max_active_per_harness = 2
    stored.version = 2
    const { wrapper, command } = await mounted(stored)

    await wrapper.find('[data-testid="caps-harness"]').setValue('')
    await wrapper.find('[data-testid="caps-lanes"]').setValue('2')
    await wrapper.find('[data-testid="save-project-caps"]').trigger('click')
    await flushPromises()

    const sent = command.mock.calls[0]?.[1] as Record<string, unknown>
    expect(sent.max_active_per_harness).toBeUndefined()
    expect(sent.max_active_lanes).toBe(2)
    expect(command).toHaveBeenCalledWith(
      'capacity.settings.update',
      expect.objectContaining({ mutation: expect.objectContaining({ optimistic_version: 2 }) }),
    )
  })

  it('reports a refused command instead of swallowing it', async () => {
    setActivePinia(createPinia())
    const refusing = {
      query: async (name: string) => {
        if (name === 'project.list') {
          return { projects: [project()] }
        }
        if (name === 'capacity.defaults.get') {
          return defaults
        }
        return { project_id: 1, caps: unsetCaps() }
      },
      command: async () => {
        throw Object.assign(
          new Error('a Project harness limit of 99 would relax the global 2'),
          { code: 'invalid_request' },
        )
      },
      subscribe: () => () => undefined,
      onConnectionChange: () => () => undefined,
    } as unknown as ShellTransport
    const store = useCapacityStore()
    await store.refresh(refusing)

    await store.saveProjectCaps(refusing)

    expect(store.error).toBe('a Project harness limit of 99 would relax the global 2')
  })
})
