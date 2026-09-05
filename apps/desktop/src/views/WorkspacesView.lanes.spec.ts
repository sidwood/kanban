import { mount, flushPromises } from '@vue/test-utils'
import { createPinia } from 'pinia'
import { describe, expect, it, vi } from 'vitest'
import type {
  LaneListResponse,
  LaneRecord,
  ProjectListResponse,
  ProjectRecord,
  WorkspaceListResponse,
  WorkspaceRecord,
} from '@kanban/contracts'
import router from '../router'
import { kanbanTransportKey } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import { useLanesStore } from '../stores/lanes'
import WorkspacesView from './WorkspacesView.vue'

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

function lane(overrides: Partial<LaneRecord> = {}): LaneRecord {
  return {
    id: 1,
    project_id: 1,
    workspace_id: null,
    ticket_id: null,
    version: 1,
    ...overrides,
  }
}

function workspace(overrides: Partial<WorkspaceRecord> = {}): WorkspaceRecord {
  return {
    id: 1,
    project_id: 1,
    path: '/workspaces/kanban.feature',
    is_seed: false,
    health: 'assigned',
    observation: {
      repository_identity: 'identity',
      checkout: 'branch',
      branch: 'feature',
      head: 'abc123',
      working_tree_clean: true,
      unique_unlanded_commits: false,
      lane_assignment: 1,
    },
    reuse: {
      reusable: false,
      clean: true,
      unassigned: false,
      free_of_unlanded_commits: true,
    },
    version: 3,
    ...overrides,
  }
}

function harness(options: {
  workspaces: WorkspaceRecord[]
  lanes: LaneRecord[]
  command?: (name: string, payload: Record<string, unknown>) => Promise<Record<string, unknown>>
}) {
  const query = vi.fn((name: string) => {
    if (name === 'project.list') {
      return Promise.resolve({ projects: [project()] } satisfies ProjectListResponse)
    }
    if (name === 'lane.list') {
      return Promise.resolve({ lanes: options.lanes } satisfies LaneListResponse)
    }
    return Promise.resolve({ workspaces: options.workspaces } satisfies WorkspaceListResponse)
  })
  const command =
    options.command ??
    (vi.fn(() => Promise.resolve({ ...lane() })) as unknown as (
      name: string,
      payload: Record<string, unknown>,
    ) => Promise<Record<string, unknown>>)
  const transport = {
    query,
    command,
    subscribe: () => () => undefined,
    onConnectionChange: () => () => undefined,
  } as unknown as ShellTransport
  return { transport, query, command }
}

async function mounted(options: Parameters<typeof harness>[0]) {
  const { transport, query, command } = harness(options)
  router.push('/projects/1/workspaces')
  await router.isReady()
  const wrapper = mount(WorkspacesView, {
    global: {
      plugins: [createPinia(), router],
      provide: { [kanbanTransportKey as symbol]: transport },
    },
  })
  await flushPromises()
  return { wrapper, transport, query, command, lanes: useLanesStore() }
}

describe('WorkspacesView lanes', () => {
  it('lists every lane with its workspace claim and held ticket', async () => {
    const { wrapper } = await mounted({
      workspaces: [workspace()],
      lanes: [
        lane({ id: 1, workspace_id: 1, ticket_id: 5, version: 3 }),
        lane({ id: 2 }),
      ],
    })

    expect(wrapper.find('[data-testid="lane-row-1"]').text()).toContain('/workspaces/kanban.feature')
    expect(wrapper.find('[data-testid="lane-row-1"]').text()).toContain('Ticket 5')
    expect(wrapper.find('[data-testid="lane-row-2"]').text()).toContain('no Workspace')
  })

  it('shows the lane chip on the workspace it claims', async () => {
    const { wrapper } = await mounted({
      workspaces: [workspace({ id: 2, observation: { ...workspace().observation, lane_assignment: 7 } })],
      lanes: [lane({ id: 7, workspace_id: 2 })],
    })

    expect(wrapper.find('[data-testid="workspace-lane-2"]').text()).toBe('Lane 7')
  })

  it('shows no lane chip on an unclaimed workspace', async () => {
    const { wrapper } = await mounted({
      workspaces: [workspace({ observation: { ...workspace().observation, lane_assignment: null }, health: 'available' })],
      lanes: [],
    })

    expect(wrapper.find('[data-testid="workspace-lane-1"]').exists()).toBe(false)
  })

  it('creates a lane for the project', async () => {
    const { wrapper, command } = await mounted({ workspaces: [], lanes: [] })

    await wrapper.find('[data-testid="lane-create"]').trigger('submit')
    await flushPromises()

    expect(command).toHaveBeenCalledWith(
      'lane.create',
      expect.objectContaining({ project_id: 1 }),
    )
  })

  it('assigns a workspace to the chosen lane with the lane version', async () => {
    const { wrapper, command } = await mounted({
      workspaces: [workspace({ observation: { ...workspace().observation, lane_assignment: null } })],
      lanes: [lane({ id: 3, version: 4 })],
    })

    await wrapper.find('[data-testid="workspace-lane-select-1"]').setValue('3')
    await wrapper.find('[data-testid="workspace-lane-assign-1"]').trigger('submit')
    await flushPromises()

    expect(command).toHaveBeenCalledWith(
      'lane.workspace.assign',
      expect.objectContaining({
        lane_id: 3,
        workspace_id: 1,
        mutation: expect.objectContaining({ optimistic_version: 4 }),
      }),
    )
  })

  it('surfaces the Seed refusal message from the command', async () => {
    const command = vi.fn((name: string) => {
      if (name === 'lane.workspace.assign') {
        return Promise.reject(new Error('the Seed Workspace can never be an execution Lane: /workspaces/kanban.seed'))
      }
      return Promise.resolve({ ...lane() })
    }) as unknown as (name: string, payload: Record<string, unknown>) => Promise<Record<string, unknown>>
    const { wrapper } = await mounted({
      workspaces: [workspace({ is_seed: true, path: '/workspaces/kanban.seed', observation: { ...workspace().observation, lane_assignment: null } })],
      lanes: [lane()],
      command,
    })

    await wrapper.find('[data-testid="workspace-lane-select-1"]').setValue('1')
    await wrapper.find('[data-testid="workspace-lane-assign-1"]').trigger('submit')
    await flushPromises()

    const error = wrapper.find('[data-testid="lane-error"]')
    expect(error.exists()).toBe(true)
    expect(error.text()).toContain('never be an execution Lane')
  })

  it('releases a lane claim through the lane version', async () => {
    const { wrapper, command } = await mounted({
      workspaces: [workspace()],
      lanes: [lane({ id: 1, workspace_id: 1, version: 5 })],
    })

    await wrapper.find('[data-testid="lane-release-1"]').trigger('submit')
    await flushPromises()

    expect(command).toHaveBeenCalledWith(
      'lane.workspace.release',
      expect.objectContaining({
        lane_id: 1,
        mutation: expect.objectContaining({ optimistic_version: 5 }),
      }),
    )
  })
})
