import { createPinia, setActivePinia } from 'pinia'
import { describe, expect, it, vi } from 'vitest'
import type { ProjectListResponse, ProjectRecord } from '@kanban/contracts'
import type { ShellTransport } from '../core/transport'
import { useProjectRegisterStore } from './project-register'

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
    counters: { plan: 0, spec: 0, ticket: 0 },
    version: 1,
    ...overrides,
  }
}

const draft = {
  code: 'CORE',
  name: 'Control plane',
  repository: '/repositories/kanban',
  seed_workspace: '/workspaces/kanban.seed',
  default_branch: 'main',
  herdr_session: 'kanban-main',
}

// A recording transport: every operation is captured, and the query
// and command answers are steerable from the test.
function harness() {
  const operations: Array<{ kind: 'query' | 'command'; name: string; request: unknown }> = []
  const query = vi.fn()
  const command = vi.fn()
  const transport = {
    query: (name: string, request: unknown) => {
      operations.push({ kind: 'query', name, request })
      return query(name, request)
    },
    command: (name: string, request: unknown) => {
      operations.push({ kind: 'command', name, request })
      return command(name, request)
    },
    subscribe: () => () => undefined,
  } as unknown as ShellTransport
  const listing = (...projects: ProjectRecord[]) =>
    query.mockImplementation(() =>
      Promise.resolve({ projects } satisfies ProjectListResponse),
    )
  return { transport, operations, query, command, listing }
}

describe('project register store', () => {
  it('refresh loads every Project through the generated client', async () => {
    setActivePinia(createPinia())
    const { transport, listing } = harness()
    listing(record(), record({ id: 2, code: 'WAVE', archived: true, version: 2 }))
    const projects = useProjectRegisterStore()

    await projects.refresh(transport)

    expect(projects.loaded).toBe(true)
    expect(projects.projects.map((entry) => entry.code)).toEqual(['CORE', 'WAVE'])
    expect(projects.error).toBeNull()
  })

  it('registering sends version zero, a fresh idempotency key, and every anchor', async () => {
    setActivePinia(createPinia())
    const { transport, operations, command, listing } = harness()
    listing()
    command.mockResolvedValue(record())
    const projects = useProjectRegisterStore()

    await projects.register(transport, draft)

    const register = operations.find((entry) => entry.name === 'project.register')
    expect(register?.kind).toBe('command')
    const request = register?.request as {
      mutation: { optimistic_version: number; idempotency_key: string }
      code: string
      repository: string
      seed_workspace: string
      default_branch: string
      herdr_session: string
      initiative_id: number | null
    }
    expect(request.code).toBe('CORE')
    expect(request.repository).toBe('/repositories/kanban')
    expect(request.seed_workspace).toBe('/workspaces/kanban.seed')
    expect(request.default_branch).toBe('main')
    expect(request.herdr_session).toBe('kanban-main')
    expect(request.initiative_id).toBeNull()
    expect(request.mutation.optimistic_version).toBe(0)
    expect(request.mutation.idempotency_key).toMatch(/[\w-]{8,}/)
    expect(projects.error).toBeNull()
  })

  it('registering carries the chosen Initiative', async () => {
    setActivePinia(createPinia())
    const { transport, operations, command, listing } = harness()
    listing()
    command.mockResolvedValue(record({ initiative_id: 3 }))
    const projects = useProjectRegisterStore()

    await projects.register(transport, { ...draft, initiative_id: 3 })

    const register = operations.find((entry) => entry.name === 'project.register')
    const request = register?.request as { initiative_id: number | null }
    expect(request.initiative_id).toBe(3)
  })

  it('archiving carries the stored version and refreshes', async () => {
    setActivePinia(createPinia())
    const { transport, operations, query, command } = harness()
    const stored = [record({ id: 7, version: 2 })]
    query.mockImplementation(() => Promise.resolve({ projects: [...stored] }))
    command.mockImplementation(async () => {
      // The core's recorded fact changed; the next listing shows it.
      stored[0] = record({ id: 7, archived: true, version: 3 })
      return stored[0]
    })
    const projects = useProjectRegisterStore()
    await projects.refresh(transport)

    await projects.archive(transport, 7)

    const archive = operations.find((entry) => entry.name === 'project.archive')
    const request = archive?.request as {
      mutation: { optimistic_version: number }
      project_id: number
    }
    expect(request.project_id).toBe(7)
    expect(request.mutation.optimistic_version).toBe(2)
    expect(projects.projects[0]?.archived).toBe(true)
  })

  it('a refused command reports the message and keeps the records', async () => {
    setActivePinia(createPinia())
    const { transport, command, listing } = harness()
    listing(record())
    command.mockRejectedValue({
      code: 'invalid_request',
      message: 'the Herdr session name `kanban-main` is already exclusive to another Project',
    })
    const projects = useProjectRegisterStore()
    await projects.refresh(transport)

    await projects.register(transport, draft)

    expect(projects.error).toBe(
      'the Herdr session name `kanban-main` is already exclusive to another Project',
    )
    expect(projects.projects).toHaveLength(1)
  })

  it('a failing refresh reports the unreachable core', async () => {
    setActivePinia(createPinia())
    const { transport, query } = harness()
    query.mockRejectedValue({ code: 'internal', message: 'the core connection is not writable' })
    const projects = useProjectRegisterStore()

    await projects.refresh(transport)

    expect(projects.loaded).toBe(false)
    expect(projects.error).toBe('the core connection is not writable')
  })
})
