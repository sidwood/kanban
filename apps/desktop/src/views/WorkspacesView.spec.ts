import { mount, flushPromises } from '@vue/test-utils'
import { createPinia } from 'pinia'
import { describe, expect, it, vi } from 'vitest'
import type { ProjectListResponse, ProjectRecord, WorkspaceListResponse, WorkspaceRecord } from '@kanban/contracts'
import router from '../router'
import { kanbanTransportKey } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import { useWorkspacesStore } from '../stores/workspaces'
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

function workspace(overrides: Partial<WorkspaceRecord> = {}): WorkspaceRecord {
  return {
    id: 1,
    project_id: 1,
    path: '/workspaces/kanban.seed',
    is_seed: true,
    health: 'available',
    observation: {
      repository_identity: 'identity',
      checkout: 'branch',
      branch: 'main',
      head: 'abc123',
      working_tree_clean: true,
      unique_unlanded_commits: false,
      lane_assignment: null,
    },
    reuse: {
      reusable: true,
      clean: true,
      unassigned: true,
      free_of_unlanded_commits: true,
    },
    version: 2,
    ...overrides,
  }
}

function harness(workspaces: WorkspaceRecord[]) {
  const query = vi.fn((name: string) => {
    if (name === 'project.list') {
      return Promise.resolve({ projects: [project()] } satisfies ProjectListResponse)
    }
    return Promise.resolve({ workspaces } satisfies WorkspaceListResponse)
  })
  const command = vi.fn(() => Promise.resolve(workspace()))
  const transport = {
    query,
    command,
    subscribe: () => () => undefined,
    onConnectionChange: () => () => undefined,
  } as unknown as ShellTransport
  return { transport, query, command }
}

async function mounted(workspaces: WorkspaceRecord[]) {
  const { transport, query, command } = harness(workspaces)
  router.push('/projects/1/workspaces')
  await router.isReady()
  const wrapper = mount(WorkspacesView, {
    global: {
      plugins: [createPinia(), router],
      provide: { [kanbanTransportKey as symbol]: transport },
    },
  })
  await flushPromises()
  return { wrapper, transport, query, command, store: useWorkspacesStore() }
}

describe('WorkspacesView', () => {
  it('lists every Workspace with its health and path', async () => {
    const { wrapper } = await mounted([
      workspace(),
      workspace({
        id: 2,
        path: '/workspaces/kanban.feature',
        is_seed: false,
        health: 'dirty',
        version: 3,
      }),
    ])

    expect(wrapper.find('[data-testid="workspace-health-1"]').text()).toBe('available')
    expect(wrapper.find('[data-testid="workspace-path-2"]').text()).toBe('/workspaces/kanban.feature')
    expect(wrapper.find('[data-testid="workspace-health-2"]').text()).toBe('dirty')
    expect(wrapper.find('[data-testid="workspace-seed-1"]').exists()).toBe(true)
  })

  it('renders a detached checkout as the closed state, never a branch', async () => {
    const { wrapper } = await mounted([
      workspace({
        id: 3,
        path: '/workspaces/kanban.detached',
        is_seed: false,
        observation: {
          repository_identity: 'identity',
          checkout: 'detached',
          branch: null,
          head: 'abc123',
          working_tree_clean: true,
          unique_unlanded_commits: false,
          lane_assignment: null,
        },
      }),
    ])

    expect(wrapper.find('[data-testid="workspace-detached-3"]').text()).toBe('detached')
    expect(wrapper.text()).not.toContain('HEAD')
  })

  it('renders observation failure separately from a dirty worktree', async () => {
    const { wrapper } = await mounted([
      workspace({ id: 2, path: '/workspaces/kanban.feature', is_seed: false, health: 'dirty' }),
      workspace({
        id: 5,
        path: '/workspaces/kanban.unreadable',
        is_seed: false,
        health: 'unobserved',
        observation: {
          repository_identity: 'identity',
          checkout: 'branch',
          branch: 'feature',
          head: 'abc123',
          working_tree_clean: null,
          unique_unlanded_commits: null,
          lane_assignment: null,
        },
      }),
    ])

    expect(wrapper.find('[data-testid="workspace-health-2"]').text()).toBe('dirty')
    expect(
      wrapper.find('[data-testid="workspace-unobserved-2"]').exists(),
      'a genuinely dirty worktree is not an observation failure',
    ).toBe(false)
    expect(wrapper.find('[data-testid="workspace-health-5"]').text()).toBe('unobserved')
    expect(wrapper.find('[data-testid="workspace-unobserved-5"]').text()).toBe(
      'observation failed',
    )
  })

  it('registers a Workspace path for the Project', async () => {
    const { wrapper, command } = await mounted([])

    await wrapper.find('[data-testid="workspace-path"]').setValue('/workspaces/kanban.feature')
    await wrapper.find('[data-testid="workspace-register"]').trigger('submit')
    await flushPromises()

    expect(command).toHaveBeenCalledWith(
      'workspace.register',
      expect.objectContaining({
        project_id: 1,
        path: '/workspaces/kanban.feature',
      }),
    )
  })

  it('observes one Workspace with its optimistic version', async () => {
    const { wrapper, command } = await mounted([workspace({ version: 4 })])

    await wrapper.find('[data-testid="workspace-observe-1"]').trigger('click')
    await flushPromises()

    expect(command).toHaveBeenCalledWith(
      'workspace.observe',
      expect.objectContaining({
        workspace_id: 1,
        mutation: expect.objectContaining({ optimistic_version: 4 }),
      }),
    )
  })
})
