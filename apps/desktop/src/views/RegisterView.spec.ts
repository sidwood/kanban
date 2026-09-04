import { mount, flushPromises } from '@vue/test-utils'
import { createPinia } from 'pinia'
import { describe, expect, it, vi } from 'vitest'
import type {
  InitiativeListResponse,
  InitiativeRecord,
  ProjectListResponse,
  ProjectRecord,
} from '@kanban/contracts'
import router from '../router'
import { kanbanTransportKey } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import { useProjectRegisterStore } from '../stores/project-register'
import RegisterView from './RegisterView.vue'

function initiative(overrides: Partial<InitiativeRecord> = {}): InitiativeRecord {
  return { id: 1, name: 'Reliability', archived: false, version: 1, ...overrides }
}

function record(overrides: Partial<ProjectRecord> = {}): ProjectRecord {
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
    counters: { plan: 2, spec: 0, ticket: 5 },
    version: 1,
    ...overrides,
  }
}

// The listings every mount loads, steerable per test.
function harness(projects: ProjectRecord[], initiatives: InitiativeRecord[]) {
  const query = vi.fn((name: string) => {
    if (name === 'initiative.list') {
      return Promise.resolve({ initiatives } satisfies InitiativeListResponse)
    }
    return Promise.resolve({ projects } satisfies ProjectListResponse)
  })
  const command = vi.fn(() => Promise.resolve(record()))
  const transport = {
    query,
    command,
    subscribe: () => () => undefined,
    onConnectionChange: () => () => undefined,
  } as unknown as ShellTransport
  return { transport, query, command }
}

async function mounted(projects: ProjectRecord[], initiatives: InitiativeRecord[] = []) {
  const { transport, query, command } = harness(projects, initiatives)
  router.push('/register')
  await router.isReady()
  const wrapper = mount(RegisterView, {
    global: {
      plugins: [createPinia(), router],
      provide: { [kanbanTransportKey as symbol]: transport },
    },
  })
  await flushPromises()
  return { wrapper, transport, query, command, store: useProjectRegisterStore() }
}

async function fillRegistration(wrapper: ReturnType<typeof mount>) {
  await wrapper.find('[data-testid="project-code"]').setValue('WAVE')
  await wrapper.find('[data-testid="project-name"]').setValue('Wave pool')
  await wrapper.find('[data-testid="project-repository"]').setValue('/repositories/wave')
  await wrapper.find('[data-testid="project-seed"]').setValue('/workspaces/wave.seed')
  await wrapper.find('[data-testid="project-branch"]').setValue('trunk')
  await wrapper.find('[data-testid="project-session"]').setValue('wave-main')
}

describe('RegisterView', () => {
  it('lists every Project with its code and counters, archived ones marked', async () => {
    const { wrapper } = await mounted([
      record(),
      record({ id: 2, code: 'WAVE', archived: true, version: 2 }),
    ])

    expect(wrapper.find('[data-testid="project-list"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="project-code-1"]').text()).toBe('CORE')
    expect(wrapper.find('[data-testid="project-name-1"]').text()).toBe('Control plane')
    expect(wrapper.find('[data-testid="project-counters-1"]').text()).toContain('P2')
    expect(wrapper.find('[data-testid="project-counters-1"]').text()).toContain('T5')
    expect(wrapper.find('[data-testid="project-archived-2"]').text()).toBe('Archived')
    expect(wrapper.find('[data-testid="project-archived-1"]').exists()).toBe(false)
  })

  it('offers no delete control anywhere', async () => {
    const { wrapper } = await mounted([record()])

    const html = wrapper.html().toLowerCase()
    expect(html).not.toContain('delete')
  })

  it('registering submits every anchor through the store', async () => {
    const { wrapper, command } = await mounted([])

    await fillRegistration(wrapper)
    await wrapper.find('[data-testid="project-register"]').trigger('submit')

    expect(command).toHaveBeenCalledWith(
      'project.register',
      expect.objectContaining({
        code: 'WAVE',
        name: 'Wave pool',
        repository: '/repositories/wave',
        seed_workspace: '/workspaces/wave.seed',
        default_branch: 'trunk',
        herdr_session: 'wave-main',
        initiative_id: null,
      }),
    )
  })

  it('registering under a chosen Initiative links it', async () => {
    const { wrapper, command } = await mounted([], [initiative()])

    await fillRegistration(wrapper)
    await wrapper.find('[data-testid="project-initiative"]').setValue(1)
    await wrapper.find('[data-testid="project-register"]').trigger('submit')

    expect(command).toHaveBeenCalledWith(
      'project.register',
      expect.objectContaining({ initiative_id: 1 }),
    )
  })

  it('registering with a blank anchor submits nothing', async () => {
    const { wrapper, command } = await mounted([])

    await wrapper.find('[data-testid="project-code"]').setValue('WAVE')
    await wrapper.find('[data-testid="project-register"]').trigger('submit')

    expect(command).not.toHaveBeenCalled()
  })

  it('archiving submits through the store', async () => {
    const { wrapper, command } = await mounted([record({ id: 4 })])

    await wrapper.find('[data-testid="project-archive-4"]').trigger('click')

    expect(command).toHaveBeenCalledWith(
      'project.archive',
      expect.objectContaining({ project_id: 4 }),
    )
  })

  it('shows the store error when a command is refused', async () => {
    const { wrapper, command, store } = await mounted([record()])
    command.mockRejectedValue({
      code: 'invalid_request',
      message: 'the Herdr session name `kanban-main` is already exclusive to another Project',
    })

    await fillRegistration(wrapper)
    await wrapper.find('[data-testid="project-register"]').trigger('submit')
    await flushPromises()

    expect(store.error).toContain('already exclusive')
    expect(wrapper.find('[data-testid="project-error"]').text()).toContain('already exclusive')
  })
})
