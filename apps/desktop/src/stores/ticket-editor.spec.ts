import { createPinia, setActivePinia } from 'pinia'
import { describe, expect, it, vi } from 'vitest'
import type { TicketListResponse, TicketRecord } from '@kanban/contracts'
import type { ShellTransport } from '../core/transport'
import {
  blankTicketDraft,
  parseStoryLinks,
  ticketCreateRequestOf,
  useTicketEditorStore,
} from './ticket-editor'

function record(overrides: Partial<TicketRecord> = {}): TicketRecord {
  return {
    id: 1,
    project_id: 4,
    number: 17,
    kind: 'implementation',
    priority: 'high',
    state: 'draft',
    spec_id: 3,
    title: null,
    slice: 'Spec authoring creates content versions end to end',
    criteria: [{ outcome: 'Specs mint unique numbers.', stories: ['CORE-S1-US1'] }],
    bug: null,
    subtype: null,
    mode: null,
    completion: [],
    scheduled_for: null,
    due: null,
    version: 1,
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

describe('ticket editor store', () => {
  it('refresh loads every ticket of the project through the generated client', async () => {
    setActivePinia(createPinia())
    const { transport, query } = harness()
    query.mockImplementation((_name: string, request: unknown) => {
      const asked = request as { project_id: number }
      return Promise.resolve({
        tickets: [
          record(),
          record({ id: 2, number: 18, kind: 'bug', title: 'Landing drops the branch', spec_id: null, slice: null, criteria: [] }),
          record({ id: 3, number: 19, kind: 'task', title: 'Archive the register', priority: 'low' }),
        ].filter((ticket) => ticket.project_id === asked.project_id),
      } satisfies TicketListResponse)
    })
    const editor = useTicketEditorStore()

    await editor.refresh(transport, 4)

    expect(editor.loaded).toBe(true)
    expect(editor.tickets.map((ticket) => ticket.number)).toEqual([17, 18, 19])
    expect(editor.error).toBeNull()
  })

  it('creating an implementation sends the spec, the slice, and parsed story links', async () => {
    setActivePinia(createPinia())
    const { transport, operations, query, command } = harness()
    query.mockResolvedValue({ tickets: [] } satisfies TicketListResponse)
    command.mockResolvedValue(record())
    const editor = useTicketEditorStore()

    const draft = blankTicketDraft()
    draft.kind = 'implementation'
    draft.priority = 'high'
    draft.specId = 3
    draft.slice = 'Spec authoring creates content versions end to end'
    draft.criteria = [{ outcome: 'Specs mint unique numbers.', stories: 'CORE-S1-US1, CORE-S1-US2' }]
    await editor.create(transport, 4, draft)

    const created = operations.find((entry) => entry.name === 'ticket.create')
    expect(created?.kind).toBe('command')
    expect(created?.request).toEqual({
      mutation: { optimistic_version: 0, idempotency_key: expect.stringMatching(/[\w-]{8,}/) },
      project_id: 4,
      kind: 'implementation',
      priority: 'high',
      spec_id: 3,
      slice: 'Spec authoring creates content versions end to end',
      criteria: [
        { outcome: 'Specs mint unique numbers.', stories: ['CORE-S1-US1', 'CORE-S1-US2'] },
      ],
    })
  })

  it('creating a bug or task sends the title and only the attachment it holds', async () => {
    setActivePinia(createPinia())
    const { transport, operations, query, command } = harness()
    query.mockResolvedValue({ tickets: [] } satisfies TicketListResponse)
    command.mockResolvedValue(record({ kind: 'bug' }))
    const editor = useTicketEditorStore()

    const bug = blankTicketDraft()
    bug.kind = 'bug'
    bug.priority = 'urgent'
    bug.title = 'Landing drops the integration branch'
    bug.actualBehaviour = 'The integration branch is dropped after a review lands.'
    bug.reporterEvidence = 'The landing log names the drop immediately after the merge.'
    await editor.create(transport, 4, bug)

    const task = blankTicketDraft()
    task.kind = 'task'
    task.title = 'Archive the old register'
    task.specId = 3
    await editor.create(transport, 4, task)

    const requests = operations
      .filter((entry) => entry.name === 'ticket.create')
      .map((entry) => entry.request)
    expect(requests[0]).toEqual({
      mutation: expect.any(Object),
      project_id: 4,
      kind: 'bug',
      priority: 'urgent',
      title: 'Landing drops the integration branch',
      actual_behaviour: 'The integration branch is dropped after a review lands.',
      reporter_evidence: 'The landing log names the drop immediately after the merge.',
    })
    expect(requests[1]).toEqual({
      mutation: expect.any(Object),
      project_id: 4,
      kind: 'task',
      priority: 'normal',
      title: 'Archive the old register',
      spec_id: 3,
      subtype: 'operational',
      mode: 'human',
      completion: [''],
    })
  })

  it('a refused creation reports the message and keeps the list', async () => {
    setActivePinia(createPinia())
    const { transport, query, command } = harness()
    query.mockResolvedValue({ tickets: [record()] } satisfies TicketListResponse)
    command.mockRejectedValue({
      code: 'invalid_request',
      message: 'an Implementation Ticket attaches to exactly one Spec',
    })
    const editor = useTicketEditorStore()
    await editor.refresh(transport, 4)

    const draft = blankTicketDraft()
    draft.kind = 'implementation'
    draft.slice = 'A slice'
    draft.criteria = [{ outcome: 'Any outcome.', stories: 'CORE-S1-US1' }]
    const landed = await editor.create(transport, 4, draft)

    expect(landed).toBe(false)
    expect(editor.error).toBe('an Implementation Ticket attaches to exactly one Spec')
    expect(editor.tickets).toHaveLength(1)
  })

  it('story links parse from one comma- or space-separated field', () => {
    expect(parseStoryLinks('CORE-S1-US1, CORE-S1-US2')).toEqual(['CORE-S1-US1', 'CORE-S1-US2'])
    expect(parseStoryLinks('  CORE-S1-US1\tCORE-S1-US2  ')).toEqual([
      'CORE-S1-US1',
      'CORE-S1-US2',
    ])
    expect(parseStoryLinks('')).toEqual([])
  })

  it('the request builder sends a fresh key per logical request', () => {
    const draft = blankTicketDraft()
    draft.title = 'A Bug'

    const first = ticketCreateRequestOf(4, draft, 'key-one')
    const second = ticketCreateRequestOf(4, draft, 'key-two')

    expect(first.mutation.idempotency_key).toBe('key-one')
    expect(second.mutation.idempotency_key).toBe('key-two')
    expect(first.mutation.optimistic_version).toBe(0)
  })
})
