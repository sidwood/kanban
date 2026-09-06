// The coverage matrix section of the planning view: one row per User
// Story of the Spec on display, every claim naming the Ticket whose
// criterion makes it, and the uncovered stories marked as gaps
// (DR-PS-18, completing the KAN-T16 diagnostics).
import { flushPromises, mount } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import { describe, expect, it } from 'vitest'
import type {
  ProjectListResponse,
  SpecCoverageMatrixResponse,
  SpecListResponse,
} from '@kanban/contracts'
import { kanbanTransportKey } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import PlanningView from './PlanningView.vue'

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
  counters: { plan: 1, spec: 2, ticket: 2 },
  version: 1,
} satisfies ProjectListResponse['projects'][number]

// The two Specs of the picked Project.
const specs: SpecListResponse = {
  specs: [
    {
      id: 1,
      project_id: 4,
      number: 1,
      name: 'Registration',
      execution: 'planned' as const,
      plan_id: 1,
      version: 3,
    },
    {
      id: 2,
      project_id: 4,
      number: 2,
      name: 'Tickets',
      execution: 'planned' as const,
      plan_id: 1,
      version: 1,
    },
  ],
}

// The matrix of Spec 1's approved version: US1 claimed by two Tickets,
// US2 by one, US3 by none.
const matrix: SpecCoverageMatrixResponse = {
  spec_id: 1,
  version: 2,
  stories: [
    {
      story: 'CORE-S1-US1',
      claims: [
        { ticket_id: 5, ticket_number: 5, outcome: 'Graphs record completely.' },
        { ticket_id: 6, ticket_number: 6, outcome: 'Claims accumulate across Tickets.' },
      ],
    },
    {
      story: 'CORE-S1-US2',
      claims: [{ ticket_id: 6, ticket_number: 6, outcome: 'Slices stay granular.' }],
    },
    { story: 'CORE-S1-US3', claims: [] },
  ],
}

// The matrix of Spec 2, read when the picker switches.
const second: SpecCoverageMatrixResponse = {
  spec_id: 2,
  version: 1,
  stories: [{ story: 'CORE-S2-US1', claims: [] }],
}

// A transport answering per operation name and recording the matrix
// reads the view issues.
function harness() {
  const reads: Array<{ spec_id: number; version: number | null }> = []
  const transport = {
    query: (name: string, request: unknown) => {
      if (name === 'project.list') {
        return Promise.resolve({ projects: [project] } satisfies ProjectListResponse)
      }
      if (name === 'spec.list') {
        return Promise.resolve(specs)
      }
      if (name === 'plan.list') {
        return Promise.resolve({ plans: [] })
      }
      if (name === 'spec.coverage.matrix') {
        const query = request as { spec_id: number; version: number | null }
        reads.push(query)
        return Promise.resolve(query.spec_id === 1 ? matrix : second)
      }
      return Promise.resolve({})
    },
    command: () => Promise.resolve({}),
    subscribe: () => () => undefined,
    onConnectionChange: () => () => undefined,
  } as unknown as ShellTransport
  return { transport, reads }
}

async function mountView(transport: ShellTransport) {
  const wrapper = mount(PlanningView, {
    global: {
      plugins: [createPinia()],
      provide: { [kanbanTransportKey as symbol]: transport },
    },
  })
  await flushPromises()
  return wrapper
}

describe('PlanningView coverage matrix', () => {
  it('renders one row per story with every claim and the gaps', async () => {
    setActivePinia(createPinia())
    const { transport } = harness()

    const wrapper = await mountView(transport)

    const section = wrapper.find('[data-testid="coverage-matrix"]')
    expect(section.exists()).toBe(true)
    expect(wrapper.find('[data-testid="coverage-version"]').text()).toBe('v2')

    const first = wrapper.find('[data-testid="coverage-row-CORE-S1-US1"]')
    expect(first.find('span.font-mono').text()).toBe('CORE-S1-US1')
    expect(first.find('[data-testid="coverage-claim-CORE-S1-US1-5"]').text()).toContain(
      'CORE-T5 — Graphs record completely.',
    )
    expect(first.find('[data-testid="coverage-claim-CORE-S1-US1-6"]').text()).toContain(
      'CORE-T6 — Claims accumulate across Tickets.',
    )

    expect(
      wrapper.find('[data-testid="coverage-row-CORE-S1-US2"]').findAll('li').length,
    ).toBe(1)
    expect(wrapper.find('[data-testid="coverage-gap-CORE-S1-US3"]').text()).toBe('uncovered')
  })

  it('re-reads the matrix when the picker switches Specs', async () => {
    setActivePinia(createPinia())
    const { transport, reads } = harness()
    const wrapper = await mountView(transport)
    expect(reads.map((read) => read.spec_id)).toEqual([1])

    const picker = wrapper.find('[data-testid="coverage-spec"]')
    await picker.setValue('2')
    await flushPromises()

    expect(reads.map((read) => read.spec_id)).toEqual([1, 2])
    expect(wrapper.find('[data-testid="coverage-version"]').text()).toBe('v1')
    expect(wrapper.find('[data-testid="coverage-row-CORE-S2-US1"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="coverage-gap-CORE-S2-US1"]').text()).toBe('uncovered')
  })

  it('stays absent while the Project holds no Specs', async () => {
    setActivePinia(createPinia())
    const empty = {
      query: (name: string) =>
        Promise.resolve(
          name === 'project.list'
            ? { projects: [project] }
            : name === 'spec.list'
              ? { specs: [] }
              : name === 'plan.list'
                ? { plans: [] }
                : {},
        ),
      command: () => Promise.resolve({}),
      subscribe: () => () => undefined,
      onConnectionChange: () => () => undefined,
    } as unknown as ShellTransport

    const wrapper = await mountView(empty)

    expect(wrapper.find('[data-testid="coverage-matrix"]').exists()).toBe(false)
  })
})
