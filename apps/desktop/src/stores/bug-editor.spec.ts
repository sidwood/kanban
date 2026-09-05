import { createPinia, setActivePinia } from 'pinia'
import { describe, expect, it, vi } from 'vitest'
import type { TicketRecord } from '@kanban/contracts'
import type { ShellTransport } from '../core/transport'
import {
  BUG_SEVERITIES,
  blankBugFactsDraft,
  blankBugQualificationDraft,
  bugFactsRequestOf,
  bugQualifyRequestOf,
  parseEvidenceIds,
  useBugEditorStore,
} from './bug-editor'

function bugRecord(overrides: Partial<TicketRecord> = {}): TicketRecord {
  return {
    id: 2,
    project_id: 4,
    number: 18,
    kind: 'bug',
    priority: 'urgent',
    state: 'draft',
    spec_id: null,
    title: 'Landing drops the integration branch',
    slice: null,
    criteria: [],
    completion: [],
    bug: {
      actual_behaviour: 'The integration branch is dropped after a review lands.',
      reporter_evidence: 'The landing log names the drop immediately after the merge.',
      external_references: [],
      occurrence_snapshots: [],
      evidence_ids: [],
    },
    version: 1,
    ...overrides,
  }
}

// A recording transport: every operation is captured, and the command
// answers are steerable from the test.
function harness() {
  const operations: Array<{ kind: 'query' | 'command'; name: string; request: unknown }> = []
  const command = vi.fn()
  const transport = {
    command: (name: string, request: unknown) => {
      operations.push({ kind: 'command', name, request })
      return command(name, request)
    },
    query: () => Promise.resolve({}),
    subscribe: () => () => undefined,
    onConnectionChange: () => () => undefined,
  } as unknown as ShellTransport
  return { transport, operations, command }
}

describe('bug editor store', () => {
  it('severity offers exactly the closed vocabulary', () => {
    expect(BUG_SEVERITIES).toEqual(['critical', 'high', 'medium', 'low'])
    expect(BUG_SEVERITIES).toContain(blankBugQualificationDraft().severity)
  })

  it('the qualification request carries the whole qualification at the read version', () => {
    const draft = blankBugQualificationDraft()
    draft.expectedBehaviour = 'The integration branch survives every landing.'
    draft.reproduction = 'Re land a reviewed change.'
    draft.environment = 'macOS 26.'
    draft.severity = 'critical'
    draft.frequency = 'Every landing.'
    draft.affectedScope = 'Landings.'
    draft.risk = 'Lost review state.'
    draft.criteria = [{ outcome: 'The branch survives.', stories: 'CORE-S1-US1, S1-US2' }]
    draft.verificationSteps = ['cargo test -p kanban-storage tickets', '  ', '']

    expect(bugQualifyRequestOf(2, draft, 3, 'key-1')).toEqual({
      mutation: { optimistic_version: 3, idempotency_key: 'key-1' },
      ticket_id: 2,
      qualification: {
        expected_behaviour: 'The integration branch survives every landing.',
        reproduction: 'Re land a reviewed change.',
        environment: 'macOS 26.',
        severity: 'critical',
        frequency: 'Every landing.',
        affected_scope: 'Landings.',
        risk: 'Lost review state.',
        criteria: [
          { outcome: 'The branch survives.', stories: ['CORE-S1-US1', 'S1-US2'] },
        ],
        verification_steps: [
          { command: 'cargo test -p kanban-storage tickets' },
        ],
      },
    })
  })

  it('the facts request carries the three collections, empty rows dropped', () => {
    const draft = blankBugFactsDraft()
    draft.externalReferences = [
      { uri: '  https://example.invalid/issues/12 ', label: ' The report ' },
      { uri: '', label: 'never mind' },
    ]
    draft.occurrenceSnapshots = [
      { observedAt: '2026-09-05T07:41:00Z', observation: 'The log shows the drop.' },
      { observedAt: '', observation: 'never mind' },
    ]
    draft.evidenceIds = '2, 5 8'

    expect(bugFactsRequestOf(2, draft, 4, 'key-2')).toEqual({
      mutation: { optimistic_version: 4, idempotency_key: 'key-2' },
      ticket_id: 2,
      external_references: [
        { uri: 'https://example.invalid/issues/12', label: 'The report' },
      ],
      occurrence_snapshots: [
        { observed_at: '2026-09-05T07:41:00Z', observation: 'The log shows the drop.' },
      ],
      evidence_ids: [2, 5, 8],
    })
  })

  it('evidence identities parse from one field, dropping non-identities', () => {
    expect(parseEvidenceIds('2, 5 8')).toEqual([2, 5, 8])
    expect(parseEvidenceIds('')).toEqual([])
    expect(parseEvidenceIds('2, zero, -1, 3.5, 9')).toEqual([2, 9])
  })

  it('qualify sends through the generated client and reports the landed record', async () => {
    setActivePinia(createPinia())
    const { transport, operations, command } = harness()
    command.mockResolvedValue(bugRecord({ version: 2 }))
    const editor = useBugEditorStore()

    const draft = blankBugQualificationDraft()
    draft.severity = 'high'
    const landed = await editor.qualify(transport, 2, 1, draft)

    expect(landed?.version).toBe(2)
    expect(editor.error).toBeNull()
    const sent = operations.find((entry) => entry.name === 'ticket.bug.qualify')
    expect(sent?.kind).toBe('command')
    expect((sent?.request as { ticket_id: number }).ticket_id).toBe(2)
    expect(
      (sent?.request as { mutation: { optimistic_version: number } }).mutation
        .optimistic_version,
    ).toBe(1)
  })

  it('a refused qualification reports the message and lands nothing', async () => {
    setActivePinia(createPinia())
    const { transport, command } = harness()
    command.mockRejectedValue({
      code: 'invalid_request',
      message: 'a Ticket environment cannot be blank',
    })
    const editor = useBugEditorStore()

    const landed = await editor.qualify(transport, 2, 1, blankBugQualificationDraft())

    expect(landed).toBeNull()
    expect(editor.error).toBe('a Ticket environment cannot be blank')
  })

  it('recording facts sends through the generated client and reports refusals', async () => {
    setActivePinia(createPinia())
    const { transport, operations, command } = harness()
    command.mockResolvedValue(bugRecord({ version: 2 }))
    const editor = useBugEditorStore()

    const landed = await editor.recordFacts(transport, 2, 1, blankBugFactsDraft())
    expect(landed?.version).toBe(2)
    expect(operations.at(-1)?.name).toBe('ticket.bug.facts')

    command.mockRejectedValue({
      code: 'invalid_request',
      message: 'evidence item 7 is not attached to ticket 2',
    })
    const refused = await editor.recordFacts(transport, 2, 2, blankBugFactsDraft())
    expect(refused).toBeNull()
    expect(editor.error).toBe('evidence item 7 is not attached to ticket 2')
  })
})
