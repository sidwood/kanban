import { createPinia, setActivePinia } from 'pinia'
import { describe, expect, it, vi } from 'vitest'
import type { ProjectListResponse, ProjectRecord } from '@kanban/contracts'
import type { ShellTransport } from '../core/transport'
import { useProjectRegisterStore } from './project-register'

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (failure: unknown) => void
  const promise = new Promise<T>((settle, refuse) => {
    resolve = settle
    reject = refuse
  })
  return { promise, resolve, reject }
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
    herdr_workspace: 'kanban.seed',
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
  herdr_workspace: 'kanban.seed',
  herdr_session: 'kanban-main',
}

const sessionlessDraft = { ...draft, herdr_session: ' ' }

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
  it('keeps the latest list response when an older request answers last', async () => {
    setActivePinia(createPinia())
    const first = deferred<ProjectListResponse>()
    const second = deferred<ProjectListResponse>()
    const query = vi.fn()
      .mockReturnValueOnce(first.promise)
      .mockReturnValueOnce(second.promise)
    const transport = {
      query,
      command: vi.fn(),
      subscribe: () => () => undefined,
    } as unknown as ShellTransport
    const projects = useProjectRegisterStore()

    const older = projects.refresh(transport)
    const newer = projects.refresh(transport)
    second.resolve({ projects: [record({ id: 2, code: 'WAVE' })] })
    await newer

    expect(projects.projects.map((entry) => entry.id)).toEqual([2])

    first.resolve({ projects: [record({ id: 1, code: 'CORE' })] })
    await older

    expect(projects.projects.map((entry) => entry.id)).toEqual([2])
    expect(projects.error).toBeNull()
  })

  it('reports loading and errors only for the latest list request', async () => {
    setActivePinia(createPinia())
    const first = deferred<ProjectListResponse>()
    const second = deferred<ProjectListResponse>()
    const query = vi.fn()
      .mockReturnValueOnce(first.promise)
      .mockReturnValueOnce(second.promise)
    const transport = {
      query,
      command: vi.fn(),
      subscribe: () => () => undefined,
    } as unknown as ShellTransport
    const projects = useProjectRegisterStore()

    const older = projects.refresh(transport)
    expect(projects.loading).toBe(true)
    const newer = projects.refresh(transport)

    second.resolve({ projects: [record({ id: 2, code: 'WAVE' })] })
    await newer
    expect(projects.loading).toBe(false)
    expect(projects.error).toBeNull()

    first.reject({ code: 'internal', message: 'obsolete Project list failure' })
    await older
    expect(projects.loading).toBe(false)
    expect(projects.error).toBeNull()
  })

  it('keeps a landed settings record when its follow-up list fails', async () => {
    setActivePinia(createPinia())
    const landed = record({ name: 'Recovery control plane', version: 2 })
    const { transport, query, command } = harness()
    query.mockRejectedValue({ code: 'internal', message: 'Project list refresh failed' })
    command.mockResolvedValue(landed)
    const projects = useProjectRegisterStore()
    projects.projects = [record()]
    projects.loaded = true

    const outcome = await projects.update(transport, 1, 1, {
      name: 'Recovery control plane',
      default_branch: 'main',
      herdr_workspace: 'kanban.seed',
      herdr_session: 'kanban-main',
      initiative_id: null,
    })

    expect(outcome).toEqual({ landed: true, refusal: null })
    expect(projects.projects[0]).toMatchObject({ name: 'Recovery control plane', version: 2 })
    expect(projects.error).toBe('Project list refresh failed')
    expect(projects.loading).toBe(false)
  })

  it('drops a list begun while a settings command was in flight', async () => {
    setActivePinia(createPinia())
    const commandAnswer = deferred<ProjectRecord>()
    const olderList = deferred<ProjectListResponse>()
    const landed = record({ name: 'Recovery control plane', version: 2 })
    const { transport, operations, query, command } = harness()
    query
      .mockReturnValueOnce(olderList.promise)
      .mockResolvedValueOnce({ projects: [landed] } satisfies ProjectListResponse)
    command.mockReturnValue(commandAnswer.promise)
    const projects = useProjectRegisterStore()
    projects.projects = [record()]
    projects.loaded = true

    const update = projects.update(transport, 1, 1, {
      name: 'Recovery control plane',
      default_branch: 'main',
      herdr_workspace: 'kanban.seed',
      herdr_session: 'kanban-main',
      initiative_id: null,
    })
    const oldRefresh = projects.refresh(transport)
    commandAnswer.resolve(landed)
    await update
    olderList.resolve({ projects: [record()] })
    await oldRefresh

    expect(operations.find((entry) => entry.name === 'project.update')?.request).toMatchObject({
      project_id: 1,
      mutation: { optimistic_version: 1 },
      name: 'Recovery control plane',
    })
    expect(projects.projects[0]).toMatchObject({ name: 'Recovery control plane', version: 2 })
    expect(projects.error).toBeNull()
    expect(projects.loading).toBe(false)
  })

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
      herdr_workspace: string
      herdr_session: string | null
      initiative_id: number | null
    }
    expect(request.code).toBe('CORE')
    expect(request.repository).toBe('/repositories/kanban')
    expect(request.seed_workspace).toBe('/workspaces/kanban.seed')
    expect(request.default_branch).toBe('main')
    expect(request.herdr_workspace).toBe('kanban.seed')
    expect(request.herdr_session).toBe('kanban-main')
    expect(request.initiative_id).toBeNull()
    expect(request.mutation.optimistic_version).toBe(0)
    expect(request.mutation.idempotency_key).toMatch(/[\w-]{8,}/)
    expect(projects.error).toBeNull()
  })

  it('registering without a session sends null for the default session', async () => {
    setActivePinia(createPinia())
    const { transport, operations, command, listing } = harness()
    listing()
    command.mockResolvedValue(record({ herdr_session: null }))
    const projects = useProjectRegisterStore()

    await projects.register(transport, sessionlessDraft)

    const register = operations.find((entry) => entry.name === 'project.register')
    const request = register?.request as { herdr_session: string | null }
    expect(request.herdr_session).toBeNull()
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
      message: 'the project code `CORE` is already registered',
    })
    const projects = useProjectRegisterStore()
    await projects.refresh(transport)

    await projects.register(transport, draft)

    expect(projects.error).toBe('the project code `CORE` is already registered')
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
