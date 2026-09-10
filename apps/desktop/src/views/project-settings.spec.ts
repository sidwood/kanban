import { flushPromises, mount } from '@vue/test-utils'
import type { VueWrapper } from '@vue/test-utils'
import { nextTick } from 'vue'
import { createPinia, setActivePinia } from 'pinia'
import { afterEach, describe, expect, it, vi } from 'vitest'
import type { InitiativeRecord, ProjectRecord } from '@kanban/contracts'
import router from '../router'
import { kanbanTransportKey } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import { useProjectRegisterStore } from '../stores/project-register'
import AppRail from '../components/shell/AppRail.vue'
import ProjectSettingsView from './ProjectSettingsView.vue'
import RegisterView from './RegisterView.vue'

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((settle) => { resolve = settle })
  return { promise, resolve }
}

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
    counters: { plan: 1, spec: 2, ticket: 9 },
    version: 4,
    ...overrides,
  }
}

const initiatives: InitiativeRecord[] = [
  { id: 2, name: 'Recovery', archived: false, version: 1 },
]

function harness(options: { projects?: ProjectRecord[]; refuse?: string } = {}) {
  const operations: Array<{ kind: 'query' | 'command'; name: string; request: unknown }> = []
  let projects = options.projects ?? [project()]
  const query = vi.fn((name: string, request: unknown) => {
    operations.push({ kind: 'query', name, request })
    switch (name) {
      case 'project.list':
        return Promise.resolve({ projects })
      case 'initiative.list':
        return Promise.resolve({ initiatives })
      default:
        return Promise.resolve({})
    }
  })
  const command = vi.fn((name: string, request: unknown) => {
    operations.push({ kind: 'command', name, request })
    if (name !== 'project.update') return Promise.resolve({})
    if (options.refuse) {
      return Promise.reject({ code: 'invalid_request', message: options.refuse })
    }
    const body = request as Record<string, unknown>
    projects = projects.map((entry) =>
      entry.id === body.project_id
        ? {
            ...entry,
            name: body.name as string,
            default_branch: body.default_branch as string,
            herdr_workspace: body.herdr_workspace as string,
            herdr_session: body.herdr_session as string | null,
            initiative_id: body.initiative_id as number | null,
            version: entry.version + 1,
          }
        : entry,
    )
    return Promise.resolve(projects.find((entry) => entry.id === body.project_id))
  })
  const transport = {
    query,
    command,
    subscribe: () => () => undefined,
    onConnectionChange: () => () => undefined,
  } as unknown as ShellTransport
  return { transport, operations }
}

const mounted: VueWrapper[] = []
afterEach(() => {
  for (const wrapper of mounted.splice(0)) wrapper.unmount()
})

async function mountAt(
  component: unknown,
  path: string,
  transport: ShellTransport,
  pinia = createPinia(),
) {
  await router.push(path)
  await router.isReady()
  const wrapper = mount(component as never, {
    global: {
      plugins: [pinia, router],
      provide: { [kanbanTransportKey as symbol]: transport },
    },
  })
  mounted.push(wrapper)
  await flushPromises()
  return wrapper
}

describe('Projects settings route', () => {
  it('is reachable from the Projects surface', async () => {
    const { transport } = harness()
    const wrapper = await mountAt(RegisterView, '/register', transport)

    expect(wrapper.get('[data-testid="project-settings-1"]').attributes('href')).toBe(
      '/projects/1/settings',
    )
  })

  it('is a route of its own and not a rail destination', async () => {
    const settings = router.getRoutes().find((record) => record.path === '/projects/:projectId/settings')
    expect(settings?.components?.default).toBe(ProjectSettingsView)

    const { transport } = harness()
    const rail = await mountAt(AppRail, '/register', transport)
    const destinations = rail
      .findAll('a')
      .map((link) => link.attributes('href'))
      .filter((href): href is string => href !== undefined)
    expect(destinations).not.toContain('/projects/1/settings')
    expect(destinations.some((href) => href.endsWith('/settings'))).toBe(false)
  })
})

describe('Project settings', () => {
  it('adopts an untouched same-Project record refreshed by the shared register', async () => {
    const current = project({ name: 'Fresh control plane', version: 5 })
    let lists = 0
    const transport = {
      query(name: string) {
        if (name === 'project.list') {
          lists += 1
          return Promise.resolve({ projects: [lists === 1 ? project() : current] })
        }
        if (name === 'initiative.list') return Promise.resolve({ initiatives })
        return Promise.resolve({})
      },
      command: () => Promise.resolve(current),
      subscribe: () => () => undefined,
      onConnectionChange: () => () => undefined,
    } as unknown as ShellTransport
    const pinia = createPinia()
    setActivePinia(pinia)
    const wrapper = await mountAt(ProjectSettingsView, '/projects/1/settings', transport, pinia)

    await useProjectRegisterStore().refresh(transport)
    await flushPromises()

    expect((wrapper.get('[data-testid="settings-name"]').element as HTMLInputElement).value)
      .toBe('Fresh control plane')
  })

  it('submits refreshed content with the version of the same record', async () => {
    const retained = project({ id: 5, code: 'EDGE', name: 'Stale edge name', version: 1 })
    const refreshed = project({ id: 5, code: 'EDGE', name: 'Fresh edge name', version: 2 })
    const first = [project(), retained]
    const current = [project(), refreshed]
    const commands: Record<string, unknown>[] = []
    let lists = 0
    const transport = {
      query(name: string) {
        if (name === 'project.list') {
          lists += 1
          return Promise.resolve({ projects: lists === 1 ? first : current })
        }
        if (name === 'initiative.list') return Promise.resolve({ initiatives })
        return Promise.resolve({})
      },
      command(_name: string, request: Record<string, unknown>) {
        commands.push(request)
        return Promise.resolve(refreshed)
      },
      subscribe: () => () => undefined,
      onConnectionChange: () => () => undefined,
    } as unknown as ShellTransport
    const wrapper = await mountAt(ProjectSettingsView, '/projects/1/settings', transport)

    await router.push('/projects/5/settings')
    await flushPromises()
    await wrapper.get('[data-testid="settings-save"]').trigger('submit')
    await flushPromises()

    expect(commands).toHaveLength(1)
    expect(commands[0]).toMatchObject({
      project_id: 5,
      mutation: { optimistic_version: 2 },
      name: 'Fresh edge name',
    })
  })

  it('keeps an edit made during refresh on its true base version', async () => {
    const retained = project({ id: 5, code: 'EDGE', name: 'Retained edge name', version: 1 })
    const refreshed = project({ id: 5, code: 'EDGE', name: 'Fresh edge name', version: 2 })
    const held = deferred<{ projects: ProjectRecord[] }>()
    const commands: Record<string, unknown>[] = []
    let lists = 0
    const transport = {
      query(name: string) {
        if (name === 'project.list') {
          lists += 1
          if (lists === 1) return Promise.resolve({ projects: [project(), retained] })
          return held.promise
        }
        if (name === 'initiative.list') return Promise.resolve({ initiatives })
        return Promise.resolve({})
      },
      command(_name: string, request: Record<string, unknown>) {
        commands.push(request)
        return Promise.reject({
          code: 'stale_version',
          message: 'expected version 1 but current version is 2',
          current_version: 2,
        })
      },
      subscribe: () => () => undefined,
      onConnectionChange: () => () => undefined,
    } as unknown as ShellTransport
    const wrapper = await mountAt(ProjectSettingsView, '/projects/1/settings', transport)

    await router.push('/projects/5/settings')
    await nextTick()
    await wrapper.get('[data-testid="settings-name"]').setValue('Operator edge name')
    held.resolve({ projects: [project(), refreshed] })
    await flushPromises()
    await wrapper.get('[data-testid="settings-save"]').trigger('submit')
    await flushPromises()

    expect(commands).toHaveLength(1)
    expect(commands[0]).toMatchObject({
      project_id: 5,
      mutation: { optimistic_version: 1 },
      name: 'Operator edge name',
    })
    expect(wrapper.get('[data-testid="settings-error"]').text()).toContain('current version is 2')
  })

  it('states the identity and the anchored paths as facts, not fields', async () => {
    const { transport } = harness()
    const wrapper = await mountAt(ProjectSettingsView, '/projects/1/settings', transport)

    expect(wrapper.get('[data-testid="settings-code"]').text()).toBe('CORE')
    expect(wrapper.get('[data-testid="settings-repository"]').text()).toBe('/repositories/kanban')
    expect(wrapper.get('[data-testid="settings-seed"]').text()).toBe('/workspaces/kanban.seed')
    for (const field of ['settings-code', 'settings-repository', 'settings-seed']) {
      const fact = wrapper.get(`[data-testid="${field}"]`)
      expect(fact.element.tagName).not.toBe('INPUT')
      expect(fact.find('input').exists()).toBe(false)
    }
  })

  it('saves the settings an operator owns against the record’s own version', async () => {
    const { transport, operations } = harness()
    const wrapper = await mountAt(ProjectSettingsView, '/projects/1/settings', transport)

    await wrapper.get('[data-testid="settings-name"]').setValue('Recovery control plane')
    await wrapper.get('[data-testid="settings-branch"]').setValue('trunk')
    await wrapper.get('[data-testid="settings-workspace"]').setValue('kanban.control')
    await wrapper.get('[data-testid="settings-session"]').setValue('kanban-control')
    await wrapper.get('[data-testid="settings-initiative"]').setValue('2')
    await wrapper.get('[data-testid="settings-save"]').trigger('submit')
    await flushPromises()

    expect(operations.filter((entry) => entry.kind === 'command')).toEqual([
      {
        kind: 'command',
        name: 'project.update',
        request: {
          mutation: { optimistic_version: 4, idempotency_key: expect.any(String) },
          project_id: 1,
          name: 'Recovery control plane',
          default_branch: 'trunk',
          herdr_workspace: 'kanban.control',
          herdr_session: 'kanban-control',
          initiative_id: 2,
        },
      },
    ])
    expect(wrapper.get('[data-testid="settings-saved"]').text()).toContain('CORE')
  })

  it('sends a blank session as the absence that selects the default session', async () => {
    const { transport, operations } = harness()
    const wrapper = await mountAt(ProjectSettingsView, '/projects/1/settings', transport)

    await wrapper.get('[data-testid="settings-session"]').setValue('   ')
    await wrapper.get('[data-testid="settings-save"]').trigger('submit')
    await flushPromises()

    const sent = operations.find((entry) => entry.kind === 'command')!.request as Record<string, unknown>
    expect(sent.herdr_session).toBeNull()
  })

  it('reports the core’s refusal and keeps the standing settings', async () => {
    const { transport } = harness({ refuse: 'a Project name cannot be blank' })
    const wrapper = await mountAt(ProjectSettingsView, '/projects/1/settings', transport)

    await wrapper.get('[data-testid="settings-name"]').setValue('   ')
    await wrapper.get('[data-testid="settings-save"]').trigger('submit')
    await flushPromises()

    expect(wrapper.get('[data-testid="settings-error"]').text()).toContain('cannot be blank')
    expect(wrapper.find('[data-testid="settings-saved"]').exists()).toBe(false)
  })

  it('offers no settings form for an archived Project', async () => {
    const { transport } = harness({ projects: [project({ archived: true })] })
    const wrapper = await mountAt(ProjectSettingsView, '/projects/1/settings', transport)

    expect(wrapper.find('[data-testid="settings-save"]').exists()).toBe(false)
    expect(wrapper.get('[data-testid="settings-archived"]').text()).toContain('terminal')
  })

  it('says so when the route names a Project the register does not hold', async () => {
    const { transport } = harness()
    const wrapper = await mountAt(ProjectSettingsView, '/projects/7/settings', transport)

    expect(wrapper.get('[data-testid="settings-missing"]').text()).toContain('7')
    expect(wrapper.find('[data-testid="settings-save"]').exists()).toBe(false)
  })

  it('follows a link that changes the Project on the mounted surface', async () => {
    const { transport } = harness({
      projects: [project(), project({ id: 5, code: 'EDGE', name: 'Edge tooling', version: 2 })],
    })
    const wrapper = await mountAt(ProjectSettingsView, '/projects/1/settings', transport)

    expect(wrapper.get('[data-testid="settings-code"]').text()).toBe('CORE')

    await router.push('/projects/5/settings')
    await flushPromises()

    expect(wrapper.get('[data-testid="settings-code"]').text()).toBe('EDGE')
    expect(
      (wrapper.get('[data-testid="settings-name"]').element as HTMLInputElement).value,
    ).toBe('Edge tooling')
  })
})
