import { flushPromises, mount } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import { describe, expect, it } from 'vitest'
import type {
  ProjectListResponse,
  SpecListResponse,
  TicketListResponse,
  TicketRecord,
} from '@kanban/contracts'
import { kanbanTransportKey } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import TicketEditorView from './TicketEditorView.vue'

const project = {
  id: 4,
  code: 'CORE',
  name: 'Control plane',
  repository: '/repositories/kanban',
  seed_workspace: '/workspaces/kanban.seed',
  default_branch: 'main',
  herdr_session: 'kanban-main',
  herdr_workspace: 'kanban.seed',
  initiative_id: null,
  archived: false,
  counters: { plan: 2, spec: 3, ticket: 1 },
  version: 1,
}

const capturedBug = {
  id: 2,
  project_id: 4,
  number: 18,
  kind: 'bug' as const,
  priority: 'urgent' as const,
  state: 'draft' as const,
  spec_id: null,
  title: 'Landing drops the integration branch',
  slice: null,
  criteria: [],
  bug: {
    actual_behaviour: 'The integration branch is dropped after a review lands.',
    reporter_evidence: 'The landing log names the drop immediately after the merge.',
    external_references: [],
    occurrence_snapshots: [],
    evidence_ids: [],
  },
  version: 1,
} satisfies TicketRecord

const qualifiedBug = {
  ...capturedBug,
  id: 19,
  number: 19,
  bug: {
    ...capturedBug.bug,
    qualification: {
      expected_behaviour: 'The integration branch survives every landing.',
      reproduction: 'Re land a reviewed change; the branch list still names it.',
      environment: 'macOS 26, Kanban 0.1.0.',
      severity: 'high' as const,
      frequency: 'Every landing so far.',
      affected_scope: 'All landing reviews.',
      risk: 'Duplicate landings and lost review state.',
      criteria: [
        { outcome: 'The integration branch survives a landing.', stories: ['CORE-S1-US1'] },
      ],
      verification_steps: [{ command: 'cargo test -p kanban-storage tickets' }],
    },
    external_references: [
      { uri: 'https://example.invalid/issues/12', label: 'The report' },
    ],
    occurrence_snapshots: [
      { observed_at: '2026-09-05T07:41:00Z', observation: 'The log shows the drop.' },
    ],
    evidence_ids: [3],
  },
  version: 3,
} satisfies TicketRecord

// A transport steered per operation name, recording every command.
function harness(tickets: TicketRecord[]) {
  const operations: Array<{ kind: 'query' | 'command'; name: string; request: unknown }> = []
  const answers: Record<string, unknown> = {
    'project.list': { projects: [project] } satisfies ProjectListResponse,
    'spec.list': { specs: [] } satisfies SpecListResponse,
    'ticket.list': { tickets } satisfies TicketListResponse,
  }
  const transport = {
    query: (name: string, request: unknown) => {
      operations.push({ kind: 'query', name, request })
      return Promise.resolve(answers[name])
    },
    command: (name: string, request: unknown) => {
      operations.push({ kind: 'command', name, request })
      if (name === 'ticket.bug.qualify') {
        return Promise.resolve({ ...qualifiedBug })
      }
      return Promise.resolve({ ...qualifiedBug, version: capturedBug.version + 1 })
    },
    subscribe: () => () => undefined,
    onConnectionChange: () => () => undefined,
  } as unknown as ShellTransport
  return { transport, operations }
}

async function mountView(transport: ShellTransport) {
  const wrapper = mount(TicketEditorView, {
    global: {
      plugins: [createPinia()],
      provide: { [kanbanTransportKey as symbol]: transport },
    },
  })
  await flushPromises()
  return wrapper
}

describe('TicketEditorView Bug qualification', () => {
  it('a captured Bug shows unqualified; a qualified Bug shows its severity', async () => {
    setActivePinia(createPinia())
    const wrapper = await mountView(harness([capturedBug, qualifiedBug]).transport)

    expect(wrapper.find('[data-testid="ticket-bug-severity-2"]').text()).toBe('unqualified')
    expect(wrapper.find('[data-testid="ticket-bug-severity-19"]').text()).toBe('high')
    const badges = wrapper.findAll('[data-testid^="ticket-bug-severity-"]')
    expect(badges.map((badge) => badge.text())).toEqual(['unqualified', 'high'])
  })

  it('picking a Bug seeds the qualification form with what stands', async () => {
    setActivePinia(createPinia())
    const wrapper = await mountView(harness([qualifiedBug]).transport)

    expect(wrapper.find('[data-testid="bug-qualify-severity"]').exists()).toBe(false)
    await wrapper.find('[data-testid="bug-pick"]').setValue('19')
    expect(wrapper.find('[data-testid="bug-qualify-severity"]').exists()).toBe(true)

    expect((wrapper.find('[data-testid="bug-qualify-severity"]').element as HTMLSelectElement).value).toBe('high')
    expect(
      (wrapper.find('[data-testid="bug-qualify-expected"]').element as HTMLInputElement).value,
    ).toContain('survives every landing')
    expect(
      (wrapper.find('[data-testid="bug-qualify-step-0"]').element as HTMLInputElement).value,
    ).toBe('cargo test -p kanban-storage tickets')
  })

  it('qualifying sends the whole qualification at the read version', async () => {
    setActivePinia(createPinia())
    const state = harness([capturedBug])
    const wrapper = await mountView(state.transport)

    await wrapper.find('[data-testid="bug-pick"]').setValue('2')
    await wrapper.find('[data-testid="bug-qualify-severity"]').setValue('critical')
    await wrapper
      .find('[data-testid="bug-qualify-expected"]')
      .setValue('The integration branch survives every landing.')
    await wrapper
      .find('[data-testid="bug-qualify-reproduction"]')
      .setValue('Re land a reviewed change.')
    await wrapper.find('[data-testid="bug-qualify-environment"]').setValue('macOS 26.')
    await wrapper.find('[data-testid="bug-qualify-frequency"]').setValue('Every landing.')
    await wrapper.find('[data-testid="bug-qualify-scope"]').setValue('All landing reviews.')
    await wrapper.find('[data-testid="bug-qualify-risk"]').setValue('Lost review state.')
    await wrapper
      .find('[data-testid="bug-qualify-criterion-outcome-0"]')
      .setValue('The integration branch survives a landing.')
    await wrapper.find('[data-testid="bug-qualify-criterion-stories-0"]').setValue('CORE-S1-US1')
    await wrapper.find('[data-testid="bug-qualify-step-0"]').setValue('cargo test -p kanban-storage tickets')
    await wrapper.find('[data-testid="bug-qualify"]').trigger('submit')
    await flushPromises()

    const qualified = state.operations.find((entry) => entry.name === 'ticket.bug.qualify')
    expect(qualified?.request).toEqual({
      mutation: expect.any(Object),
      ticket_id: 2,
      qualification: {
        expected_behaviour: 'The integration branch survives every landing.',
        reproduction: 'Re land a reviewed change.',
        environment: 'macOS 26.',
        severity: 'critical',
        frequency: 'Every landing.',
        affected_scope: 'All landing reviews.',
        risk: 'Lost review state.',
        criteria: [
          {
            outcome: 'The integration branch survives a landing.',
            stories: ['CORE-S1-US1'],
          },
        ],
        verification_steps: [{ command: 'cargo test -p kanban-storage tickets' }],
      },
    })
  })

  it('a refused qualification reports the message', async () => {
    setActivePinia(createPinia())
    const state = harness([capturedBug])
    state.transport.command = ((
      name: string,
      request: unknown,
    ): Promise<TicketRecord> => {
      state.operations.push({ kind: 'command', name, request })
      return Promise.reject({
        code: 'invalid_request',
        message: 'a Ticket environment cannot be blank',
      })
    }) as unknown as ShellTransport['command']
    const wrapper = await mountView(state.transport)

    await wrapper.find('[data-testid="bug-pick"]').setValue('2')
    await wrapper.find('[data-testid="bug-qualify"]').trigger('submit')
    await flushPromises()

    expect(wrapper.find('[data-testid="bug-error"]').text()).toBe(
      'a Ticket environment cannot be blank',
    )
  })

  it('recording facts sends the three collections at the read version', async () => {
    setActivePinia(createPinia())
    const state = harness([capturedBug])
    const wrapper = await mountView(state.transport)

    await wrapper.find('[data-testid="bug-pick"]').setValue('2')
    await wrapper
      .find('[data-testid="bug-facts-reference-uri-0"]')
      .setValue('https://example.invalid/issues/12')
    await wrapper.find('[data-testid="bug-facts-reference-label-0"]').setValue('The report')
    await wrapper
      .find('[data-testid="bug-facts-snapshot-at-0"]')
      .setValue('2026-09-05T07:41:00Z')
    await wrapper
      .find('[data-testid="bug-facts-snapshot-observation-0"]')
      .setValue('The log shows the drop.')
    await wrapper.find('[data-testid="bug-facts-evidence"]').setValue('3, 7')
    await wrapper.find('[data-testid="bug-facts"]').trigger('submit')
    await flushPromises()

    const recorded = state.operations.find((entry) => entry.name === 'ticket.bug.facts')
    expect(recorded?.request).toEqual({
      mutation: expect.any(Object),
      ticket_id: 2,
      external_references: [
        { uri: 'https://example.invalid/issues/12', label: 'The report' },
      ],
      occurrence_snapshots: [
        { observed_at: '2026-09-05T07:41:00Z', observation: 'The log shows the drop.' },
      ],
      evidence_ids: [3, 7],
    })
  })
})
