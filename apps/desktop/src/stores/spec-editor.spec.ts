import { createPinia, setActivePinia } from 'pinia'
import { describe, expect, it, vi } from 'vitest'
import type {
  SpecContent,
  SpecGetResponse,
  SpecListResponse,
  SpecRecord,
  SpecVersionRecord,
} from '@kanban/contracts'
import type { ShellTransport } from '../core/transport'
import { diffContent, useSpecEditorStore } from './spec-editor'

function content(overrides: Partial<SpecContent> = {}): SpecContent {
  return {
    name: 'Plans and specifications',
    short_description: 'Versioned Plan graphs of Specs',
    problem_statement: 'Planning must survive change.',
    solution: 'Immutable approved versions.',
    user_stories: 'KAN-S3-US4',
    implementation_decisions: 'Supersession is explicit.',
    testing_decisions: 'Domain tests prove immutability.',
    out_of_scope: 'The Ticket graph proposal.',
    further_notes: 'None',
    ...overrides,
  }
}

function record(overrides: Partial<SpecRecord> = {}): SpecRecord {
  return {
    id: 1,
    project_id: 4,
    number: 1,
    name: 'Plans and specifications',
    execution: 'unplanned',
    plan_id: null,
    version: 3,
    ...overrides,
  }
}

function version(number: number, overrides: Partial<SpecVersionRecord> = {}): SpecVersionRecord {
  return {
    number,
    state: 'draft',
    content: content(),
    ...overrides,
  }
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
  return { transport, operations, query, command }
}

// Point the transport's query answers at one open Spec with its
// versions.
function serving(
  query: ReturnType<typeof harness>['query'],
  spec: SpecRecord,
  versions: SpecVersionRecord[],
) {
  query.mockImplementation((name: string) => {
    if (name === 'spec.get') {
      return Promise.resolve({ spec, versions } satisfies SpecGetResponse)
    }
    return Promise.resolve({ specs: [spec] } satisfies SpecListResponse)
  })
}

describe('spec editor store', () => {
  it('refresh loads every spec of the project through the generated client', async () => {
    setActivePinia(createPinia())
    const { transport, query } = harness()
    query.mockImplementation((_name: string, request: unknown) => {
      const asked = request as { project_id: number }
      return Promise.resolve({
        specs: [
          record(),
          record({ id: 2, number: 2, name: 'Timeline', execution: 'complete', version: 9 }),
          record({ id: 3, number: 3, name: 'Review', execution: 'cancelled', version: 10 }),
        ].filter((spec) => spec.project_id === asked.project_id),
      } satisfies SpecListResponse)
    })
    const editor = useSpecEditorStore()

    await editor.refresh(transport, 4)

    expect(editor.loaded).toBe(true)
    expect(editor.specs.map((spec) => spec.number)).toEqual([1, 2, 3])
    expect(editor.error).toBeNull()
  })

  it('opening loads the versions and shows the working content first', async () => {
    setActivePinia(createPinia())
    const spec = record({ version: 5 })
    const { transport, query } = harness()
    serving(query, spec, [
      version(1, { state: 'superseded', content: content({ name: 'Registration' }) }),
      version(2, { state: 'approved' }),
      version(3, { state: 'draft', content: content({ name: 'Registration, revised' }) }),
    ])
    const editor = useSpecEditorStore()

    await editor.open(transport, 1)

    expect(editor.selectedSpecId).toBe(1)
    expect(editor.versions.map((held) => held.number)).toEqual([1, 2, 3])
    expect(editor.displayed?.number).toBe(3) // the working content shows first
    expect(editor.draft?.number).toBe(3)
    expect(editor.approvedVersion?.number).toBe(2)
  })

  it('the approve gate follows the explicit-supersession rule', async () => {
    setActivePinia(createPinia())
    const spec = record({ version: 5 })
    const { transport, query } = harness()
    serving(query, spec, [version(1, { state: 'approved' }), version(2, { state: 'draft' })])
    const editor = useSpecEditorStore()
    await editor.open(transport, 1)

    expect(editor.canApprove).toBe(false) // a still-approved version blocks approval

    const withSuperseded = record({ version: 7 })
    serving(query, withSuperseded, [version(1, { state: 'superseded' }), version(2, { state: 'draft' })])
    await editor.open(transport, 1)

    expect(editor.canApprove).toBe(true)
  })

  it('version switching and the diff compare two versions section by section', async () => {
    setActivePinia(createPinia())
    const spec = record({ version: 6 })
    const { transport, query } = harness()
    serving(query, spec, [
      version(1, {
        state: 'approved',
        content: content({
          name: 'Registration',
          user_stories: 'KAN-S3-US4\nKAN-S3-US5',
          solution: 'Immutable approved versions.',
        }),
      }),
      version(2, {
        state: 'draft',
        content: content({
          name: 'Registration, revised',
          user_stories: 'KAN-S3-US4',
          solution: 'Immutable approved versions, explicitly superseded.',
        }),
      }),
    ])
    const editor = useSpecEditorStore()
    await editor.open(transport, 1)

    editor.showVersion(1)
    expect(editor.displayed?.number).toBe(1)

    editor.compareWith(2)
    expect(editor.compared?.number).toBe(2)
    // Removed is what only the compared baseline held; added is what
    // only the displayed version holds.
    expect(editor.diff).toEqual([
      { section: 'name', removed: ['Registration, revised'], added: ['Registration'] },
      { section: 'short_description', removed: [], added: [] },
      { section: 'problem_statement', removed: [], added: [] },
      {
        section: 'solution',
        removed: ['Immutable approved versions, explicitly superseded.'],
        added: ['Immutable approved versions.'],
      },
      { section: 'user_stories', removed: [], added: ['KAN-S3-US5'] },
      { section: 'implementation_decisions', removed: [], added: [] },
      { section: 'testing_decisions', removed: [], added: [] },
      { section: 'out_of_scope', removed: [], added: [] },
      { section: 'further_notes', removed: [], added: [] },
    ])

    editor.compareWith(2)
    expect(editor.comparedVersion).toBeNull() // the same number clears the comparison
    expect(editor.diff).toBeNull()
  })

  it('the diff counts repeated lines once each time they appear', () => {
    const before = content({ solution: 'One rule.\nOne rule.\nKept.' })
    const after = content({ solution: 'One rule.\nKept.' })

    expect(diffContent(before, after).find((entry) => entry.section === 'solution')).toEqual({
      section: 'solution',
      removed: ['One rule.'],
      added: [],
    })
  })

  it('creating sends version zero, a fresh idempotency key, and the content', async () => {
    setActivePinia(createPinia())
    const fresh = record({ version: 1 })
    const { transport, operations, query, command } = harness()
    serving(query, fresh, [version(1)])
    command.mockResolvedValue(fresh)
    const editor = useSpecEditorStore()
    await editor.refresh(transport, 4)

    await editor.create(transport, 4, content())

    const created = operations.find((entry) => entry.name === 'spec.create')
    expect(created?.kind).toBe('command')
    const request = created?.request as {
      mutation: { optimistic_version: number; idempotency_key: string }
      project_id: number
      content: SpecContent
    }
    expect(request.project_id).toBe(4)
    expect(request.content.name).toBe('Plans and specifications')
    expect(request.mutation.optimistic_version).toBe(0)
    expect(request.mutation.idempotency_key).toMatch(/[\w-]{8,}/)
  })

  it('content updates carry the stored version and the nine sections', async () => {
    setActivePinia(createPinia())
    const spec = record({ version: 5 })
    const { transport, operations, query, command } = harness()
    serving(query, spec, [version(1)])
    command.mockResolvedValue(spec)
    const editor = useSpecEditorStore()
    await editor.open(transport, 1)

    await editor.updateContent(transport, content({ name: 'Revised' }))

    const update = operations.find((entry) => entry.name === 'spec.content.update')
    const request = update?.request as {
      mutation: { optimistic_version: number }
      spec_id: number
      content: SpecContent
    }
    expect(request.spec_id).toBe(1)
    expect(request.mutation.optimistic_version).toBe(5)
    expect(Object.keys(request.content).sort()).toEqual(
      [
        'further_notes',
        'implementation_decisions',
        'name',
        'out_of_scope',
        'problem_statement',
        'short_description',
        'solution',
        'testing_decisions',
        'user_stories',
      ].sort(),
    )
  })

  it('approve and supersede name their targets', async () => {
    setActivePinia(createPinia())
    const spec = record({ version: 5 })
    const { transport, operations, query, command } = harness()
    serving(query, spec, [version(1, { state: 'approved' })])
    command.mockResolvedValue(spec)
    const editor = useSpecEditorStore()
    await editor.open(transport, 1)

    await editor.approve(transport)
    await editor.supersede(transport, 1)

    const approve = operations.find((entry) => entry.name === 'spec.version.approve')
    expect(approve?.request).toMatchObject({ spec_id: 1 })
    const supersede = operations.find((entry) => entry.name === 'spec.version.supersede')
    expect(supersede?.request).toMatchObject({ spec_id: 1, version: 1 })
  })

  it('planning and execution moves send their targets', async () => {
    setActivePinia(createPinia())
    const planned = record({ execution: 'planned', plan_id: 2, version: 6 })
    const { transport, operations, query, command } = harness()
    serving(query, planned, [version(1)])
    command.mockResolvedValue(planned)
    const editor = useSpecEditorStore()
    await editor.open(transport, 1)

    await editor.joinPlan(transport, 2)
    await editor.moveExecution(transport, 'ready')

    const join = operations.find((entry) => entry.name === 'spec.plan.join')
    expect(join?.request).toMatchObject({ spec_id: 1, plan_id: 2 })
    const moved = operations.find((entry) => entry.name === 'spec.execution.move')
    expect(moved?.request).toMatchObject({ spec_id: 1, execution: 'ready' })
  })

  it('a refused command reports the message', async () => {
    setActivePinia(createPinia())
    const spec = record({ version: 5 })
    const { transport, query, command } = harness()
    serving(query, spec, [version(1, { state: 'approved' }), version(2, { state: 'draft' })])
    command.mockRejectedValue({
      code: 'invalid_request',
      message: 'version 1 is still approved; supersede it before approving another',
    })
    const editor = useSpecEditorStore()
    await editor.open(transport, 1)

    await editor.approve(transport)

    expect(editor.error).toBe(
      'version 1 is still approved; supersede it before approving another',
    )
  })
})
