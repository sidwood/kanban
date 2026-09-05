// The bounded Task half of the Ticket editor (KAN-S4-US4): a Task
// drafts one subtype of the closed set, a human-or-agent mode, its
// completion criteria, and optional schedule or due-date timing, and
// sends exactly those fields — never story-linked criteria — through
// the generated client.
import { createPinia, setActivePinia } from 'pinia'
import { describe, expect, it, vi } from 'vitest'
import type { TicketListResponse, TicketRecord } from '@kanban/contracts'
import type { ShellTransport } from '../core/transport'
import {
  TASK_MODES,
  TASK_SUBTYPES,
  blankTicketDraft,
  ticketCreateRequestOf,
  useTicketEditorStore,
} from './ticket-editor'

function taskRecord(overrides: Partial<TicketRecord> = {}): TicketRecord {
  return {
    id: 5,
    project_id: 4,
    number: 19,
    kind: 'task',
    priority: 'low',
    state: 'draft',
    spec_id: null,
    title: 'Archive the old register',
    slice: null,
    criteria: [],
    subtype: 'migration',
    mode: 'agent',
    completion: ['The register moves.', 'The archive restores.'],
    scheduled_for: '2026-10-01T00:00:00.000Z',
    due: '2026-09-30T17:00:00.000Z',
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

// A bounded Task draft with every field filled.
function filledTaskDraft() {
  const draft = blankTicketDraft()
  draft.kind = 'task'
  draft.priority = 'low'
  draft.title = 'Archive the old register'
  draft.subtype = 'migration'
  draft.mode = 'agent'
  draft.completion = ['The register moves.', 'The archive restores.']
  draft.scheduledFor = '2026-10-01T02:00:00+02:00'
  draft.due = '2026-09-30T17:00:00Z'
  return draft
}

describe('task editor', () => {
  it('offers the closed subtype and mode vocabularies', () => {
    expect(TASK_SUBTYPES).toEqual([
      'operational',
      'investigative',
      'administrative',
      'research',
      'prototype',
      'migration',
      'manual',
    ])
    expect(TASK_MODES).toEqual(['human', 'agent'])
  })

  it('a blank draft defaults the first subtype and the human mode', () => {
    const draft = blankTicketDraft()

    expect(draft.subtype).toBe('operational')
    expect(draft.mode).toBe('human')
    expect(draft.completion).toEqual([''])
    expect(draft.scheduledFor).toBe('')
    expect(draft.due).toBe('')
  })

  it('building a task request sends the subtype, mode, completion, and timing', () => {
    const request = ticketCreateRequestOf(4, filledTaskDraft(), 'key-task')

    expect(request).toMatchObject({
      project_id: 4,
      kind: 'task',
      priority: 'low',
      title: 'Archive the old register',
      subtype: 'migration',
      mode: 'agent',
      completion: ['The register moves.', 'The archive restores.'],
      scheduled_for: '2026-10-01T02:00:00+02:00',
      due: '2026-09-30T17:00:00Z',
    })
    expect(request).not.toHaveProperty('slice')
    expect(request).not.toHaveProperty('criteria')
    expect(request.mutation.optimistic_version).toBe(0)
  })

  it('blank timing stays absent from a task request', () => {
    const draft = filledTaskDraft()
    draft.scheduledFor = ''
    draft.due = '   '

    const request = ticketCreateRequestOf(4, draft, 'key-task')

    expect(request).not.toHaveProperty('scheduled_for')
    expect(request).not.toHaveProperty('due')
  })

  it('creating a task lands through the generated client and reports the record', async () => {
    setActivePinia(createPinia())
    const { transport, operations, query, command } = harness()
    query.mockResolvedValue({ tickets: [taskRecord()] } satisfies TicketListResponse)
    command.mockResolvedValue(taskRecord())
    const editor = useTicketEditorStore()

    const landed = await editor.create(transport, 4, filledTaskDraft())

    expect(landed).toBe(true)
    const created = operations.find((entry) => entry.name === 'ticket.create')
    expect(created?.kind).toBe('command')
    expect(created?.request).toMatchObject({
      kind: 'task',
      subtype: 'migration',
      mode: 'agent',
    })
    expect(editor.error).toBeNull()
    expect(editor.tickets.map((ticket) => ticket.subtype)).toEqual(['migration'])
  })

  it('a refused task creation reports the message and keeps the list', async () => {
    setActivePinia(createPinia())
    const { transport, query, command } = harness()
    query.mockResolvedValue({ tickets: [taskRecord()] } satisfies TicketListResponse)
    command.mockRejectedValue({
      code: 'invalid_request',
      message: 'a Task Ticket carries completion criteria',
    })
    const editor = useTicketEditorStore()
    await editor.refresh(transport, 4)

    const draft = filledTaskDraft()
    draft.completion = []
    const landed = await editor.create(transport, 4, draft)

    expect(landed).toBe(false)
    expect(editor.error).toBe('a Task Ticket carries completion criteria')
    expect(editor.tickets).toHaveLength(1)
  })
})
