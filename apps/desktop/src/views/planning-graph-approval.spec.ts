import { flushPromises, mount } from '@vue/test-utils'
import type { VueWrapper } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import { afterEach, describe, expect, it } from 'vitest'
import type {
  ProjectListResponse,
  SpecCoverageMatrixResponse,
  SpecListResponse,
  TicketGraphRecord,
  TicketListResponse,
  TicketRecord,
} from '@kanban/contracts'
import { kanbanTransportKey } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import router from '../router'
import PlanningView from './PlanningView.vue'

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((settle) => { resolve = settle })
  return { promise, resolve }
}

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

const specs: SpecListResponse = {
  specs: [
    { id: 1, project_id: 4, number: 1, name: 'Registration', execution: 'planned', plan_id: 1, version: 3 },
    { id: 2, project_id: 4, number: 2, name: 'Tickets', execution: 'planned', plan_id: 1, version: 1 },
  ],
}

function ticket(overrides: Partial<TicketRecord>): TicketRecord {
  return {
    id: 5,
    project_id: 4,
    number: 5,
    kind: 'implementation',
    priority: 'normal',
    state: 'draft',
    spec_id: 1,
    slice: 'Record graphs completely.',
    criteria: [],
    bug: null,
    completion: [],
    pinned_spec_version: null,
    version: 1,
    ...overrides,
  } as TicketRecord
}

const uncoveredMatrix: SpecCoverageMatrixResponse = {
  spec_id: 1,
  version: 2,
  stories: [
    { story: 'CORE-S1-US1', claims: [{ ticket_id: 5, ticket_number: 5, outcome: 'Graphs record.' }] },
    { story: 'CORE-S1-US2', claims: [{ ticket_id: 6, ticket_number: 6, outcome: 'Slices stay granular.' }] },
    { story: 'CORE-S1-US3', claims: [] },
  ],
}

function proposal(overrides: Partial<TicketGraphRecord> = {}): TicketGraphRecord {
  return {
    id: 11,
    spec_id: 1,
    spec_version: 2,
    tickets: [5, 6],
    edges: [{ from_ticket: 5, to_ticket: 6 }],
    state: 'proposed',
    version: 1,
    ...overrides,
  }
}

interface HarnessOptions {
  refuse?: string
  matrix?: SpecCoverageMatrixResponse
}

function harness(options: HarnessOptions = {}) {
  const operations: Array<{ kind: 'query' | 'command'; name: string; request: unknown }> = []
  let proposals: TicketGraphRecord[] = [proposal()]
  let tickets: TicketRecord[] = [
    ticket({ id: 5, number: 5 }),
    ticket({ id: 6, number: 6, slice: 'Keep slices granular.' }),
  ]
  const transport = {
    query: (name: string, request: unknown) => {
      operations.push({ kind: 'query', name, request })
      switch (name) {
        case 'project.list':
          return Promise.resolve({ projects: [project] } satisfies ProjectListResponse)
        case 'spec.list':
          return Promise.resolve(specs)
        case 'plan.list':
          return Promise.resolve({ plans: [] })
        case 'ticket.list':
          return Promise.resolve({ tickets } satisfies TicketListResponse)
        case 'ticket.graph.list':
          return Promise.resolve({ proposals })
        case 'spec.coverage.matrix':
          return Promise.resolve(options.matrix ?? uncoveredMatrix)
        default:
          return Promise.resolve({})
      }
    },
    command: (name: string, request: unknown) => {
      operations.push({ kind: 'command', name, request })
      if (name !== 'ticket.graph.approve') return Promise.resolve({})
      if (options.refuse) {
        return Promise.reject({ code: 'invalid_request', message: options.refuse })
      }
      const body = request as { proposal_id: number }
      proposals = proposals.map((entry) =>
        entry.id === body.proposal_id
          ? { ...entry, state: 'approved' as const, version: entry.version + 1 }
          : entry,
      )
      tickets = tickets.map((entry) => ({ ...entry, pinned_spec_version: 2 }))
      return Promise.resolve(proposals.find((entry) => entry.id === body.proposal_id))
    },
    subscribe: () => () => undefined,
    onConnectionChange: () => () => undefined,
  } as unknown as ShellTransport
  return { transport, operations }
}

const mounted: VueWrapper[] = []
afterEach(() => {
  for (const wrapper of mounted.splice(0)) wrapper.unmount()
})

async function mountPlanning(transport: ShellTransport, path = '/planning') {
  await router.push(path)
  await router.isReady()
  setActivePinia(createPinia())
  const wrapper = mount(PlanningView, {
    global: {
      plugins: [createPinia(), router],
      provide: { [kanbanTransportKey as symbol]: transport },
    },
  })
  mounted.push(wrapper)
  await flushPromises()
  return wrapper
}

describe('planning graph approval', () => {
  it('removes the previous Spec approval before the picked Spec read settles', async () => {
    const held = deferred<SpecCoverageMatrixResponse>()
    const commands: Array<{ name: string; request: unknown }> = []
    const newProposal = proposal({ id: 22, spec_id: 2, spec_version: 1 })
    const transport = {
      query(name: string, request: unknown) {
        const asked = request as { spec_id?: number }
        if (name === 'project.list') return Promise.resolve({ projects: [project] })
        if (name === 'spec.list') return Promise.resolve(specs)
        if (name === 'plan.list') return Promise.resolve({ plans: [] })
        if (name === 'ticket.list') return Promise.resolve({ tickets: [] })
        if (name === 'ticket.graph.list') {
          return Promise.resolve({ proposals: asked.spec_id === 1 ? [proposal()] : [newProposal] })
        }
        if (name === 'spec.coverage.matrix') {
          if (asked.spec_id === 2) return held.promise
          return Promise.resolve(uncoveredMatrix)
        }
        return Promise.resolve({})
      },
      command(name: string, request: unknown) {
        commands.push({ name, request })
        return Promise.resolve(proposal({ state: 'approved', version: 2 }))
      },
      subscribe: () => () => undefined,
      onConnectionChange: () => () => undefined,
    } as unknown as ShellTransport
    const wrapper = await mountPlanning(transport)

    void wrapper.get('[data-testid="coverage-spec"]').setValue('2')
    await wrapper.vm.$nextTick()
    const staleApproval = wrapper.find('[data-testid="graph-approve-11"]')
    expect.soft(staleApproval.exists()).toBe(false)
    expect.soft(wrapper.find('[data-testid="coverage-version"]').exists()).toBe(false)
    if (staleApproval.exists()) await staleApproval.trigger('click')

    expect(commands).toEqual([])

    held.resolve({ spec_id: 2, version: 1, stories: [] })
    await flushPromises()
    expect(wrapper.find('[data-testid="graph-proposal-22"]').exists()).toBe(true)
  })

  it('keeps an in-flight approval on its original Spec without stale follow-up reads', async () => {
    const held = deferred<TicketGraphRecord>()
    const commands: Array<{ name: string; request: unknown }> = []
    const queries: Array<{ name: string; request: unknown }> = []
    const newProposal = proposal({ id: 22, spec_id: 2, spec_version: 1 })
    const transport = {
      query(name: string, request: unknown) {
        queries.push({ name, request })
        const asked = request as { spec_id?: number }
        if (name === 'project.list') return Promise.resolve({ projects: [project] })
        if (name === 'spec.list') return Promise.resolve(specs)
        if (name === 'plan.list') return Promise.resolve({ plans: [] })
        if (name === 'ticket.list') return Promise.resolve({ tickets: [] })
        if (name === 'ticket.graph.list') {
          return Promise.resolve({ proposals: asked.spec_id === 1 ? [proposal()] : [newProposal] })
        }
        if (name === 'spec.coverage.matrix') {
          return Promise.resolve({ spec_id: asked.spec_id, version: 1, stories: [] })
        }
        return Promise.resolve({})
      },
      command(name: string, request: unknown) {
        commands.push({ name, request })
        return held.promise
      },
      subscribe: () => () => undefined,
      onConnectionChange: () => () => undefined,
    } as unknown as ShellTransport
    const wrapper = await mountPlanning(transport)

    await wrapper.get('[data-testid="graph-approve-11"]').trigger('click')
    await wrapper.get('[data-testid="coverage-spec"]').setValue('2')
    await flushPromises()
    held.resolve(proposal({ state: 'approved', version: 2 }))
    await flushPromises()

    expect(commands).toEqual([
      expect.objectContaining({
        name: 'ticket.graph.approve',
        request: expect.objectContaining({ proposal_id: 11 }),
      }),
    ])
    expect(wrapper.find('[data-testid="graph-proposal-22"]').exists()).toBe(true)
    expect(
      queries
        .filter((entry) => entry.name === 'ticket.graph.list')
        .map((entry) => (entry.request as { spec_id: number }).spec_id),
    ).toEqual([1, 2])
    expect(queries.filter((entry) => entry.name === 'ticket.list')).toHaveLength(1)
  })

  it('renders the proposal context of the Spec on display', async () => {
    const { transport } = harness()
    const wrapper = await mountPlanning(transport)

    const panel = wrapper.get('[data-testid="graph-proposals"]')
    const row = panel.get('[data-testid="graph-proposal-11"]')
    expect(row.text()).toContain('v2')
    expect(row.get('[data-testid="graph-members-11"]').text()).toContain('CORE-T5')
    expect(row.get('[data-testid="graph-members-11"]').text()).toContain('CORE-T6')
    expect(row.get('[data-testid="graph-edges-11"]').text()).toBe('CORE-T5 → CORE-T6')
    expect(row.get('[data-testid="graph-state-11"]').text()).toBe('proposed')
  })

  it('names the coverage the gate would refuse the graph for, on its own version', async () => {
    const { transport } = harness()
    const wrapper = await mountPlanning(transport)

    const warning = wrapper.get('[data-testid="graph-blocking-11"]')
    expect(warning.text()).toContain('CORE-S1-US3')
    expect(warning.text()).toContain('v2')
  })

  it('approves through the production command against the proposal’s own version', async () => {
    const { transport, operations } = harness()
    const wrapper = await mountPlanning(transport)

    await wrapper.get('[data-testid="graph-approve-11"]').trigger('click')
    await flushPromises()

    expect(operations.filter((entry) => entry.kind === 'command')).toEqual([
      {
        kind: 'command',
        name: 'ticket.graph.approve',
        request: {
          mutation: { optimistic_version: 1, idempotency_key: expect.any(String) },
          proposal_id: 11,
        },
      },
    ])
    expect(wrapper.get('[data-testid="graph-state-11"]').text()).toBe('approved')
  })

  it('reports the gate’s refusal and leaves the proposal standing', async () => {
    const { transport } = harness({ refuse: 'the graph leaves CORE-S1-US3 uncovered' })
    const wrapper = await mountPlanning(transport)

    await wrapper.get('[data-testid="graph-approve-11"]').trigger('click')
    await flushPromises()

    expect(wrapper.get('[data-testid="graph-refusal-11"]').text()).toContain('CORE-S1-US3')
    expect(wrapper.get('[data-testid="graph-state-11"]').text()).toBe('proposed')
  })

  it('shows every member’s pin once the approval has landed and the surface reloads', async () => {
    const { transport } = harness()
    const wrapper = await mountPlanning(transport)
    await wrapper.get('[data-testid="graph-approve-11"]').trigger('click')
    await flushPromises()

    const reloaded = await mountPlanning(transport)

    expect(reloaded.get('[data-testid="graph-state-11"]').text()).toBe('approved')
    expect(reloaded.get('[data-testid="graph-members-11"]').text()).toContain('pinned v2')
    expect(wrapper.exists()).toBe(true)
  })

  it('opens the exact Spec a link names', async () => {
    const { transport } = harness()
    const wrapper = await mountPlanning(transport, '/planning?project=4&spec=2')

    expect(
      (wrapper.get('[data-testid="coverage-spec"]').element as HTMLSelectElement).value,
    ).toBe('2')
  })
})
