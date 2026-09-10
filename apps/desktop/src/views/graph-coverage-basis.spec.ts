// Each Ticket graph proposal is judged on its own Spec content version
// (KAN-T140-AC1, DR-PS-14, DR-PS-17). The gate refuses a graph on the
// coverage of the version it was recorded against, so the surface must
// read that version's coverage — not whatever version the Spec is
// operating at now — and say which version each warning is about.
import { flushPromises, mount } from '@vue/test-utils'
import type { VueWrapper } from '@vue/test-utils'
import { createPinia } from 'pinia'
import { afterEach, describe, expect, it } from 'vitest'
import type {
  ProjectRecord,
  SpecCoverageMatrixResponse,
  TicketGraphRecord,
} from '@kanban/contracts'
import router from '../router'
import { kanbanTransportKey } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import PlanningView from './PlanningView.vue'

const project: ProjectRecord = {
  id: 4,
  code: 'CORE',
  name: 'Control plane',
  repository: '/repositories/kanban',
  seed_workspace: '/workspaces/kanban.seed',
  default_branch: 'main',
  herdr_session: null,
  herdr_workspace: 'kanban.seed',
  initiative_id: null,
  archived: false,
  counters: { plan: 0, spec: 2, ticket: 2 },
  version: 1,
}

const specs = [
  { id: 1, project_id: 4, number: 1, name: 'Registration', execution: 'planned', plan_id: null, version: 2 },
  { id: 9, project_id: 4, number: 2, name: 'Tickets', execution: 'planned', plan_id: null, version: 1 },
]

function proposal(overrides: Partial<TicketGraphRecord> = {}): TicketGraphRecord {
  return {
    id: 11,
    spec_id: 1,
    spec_version: 1,
    tickets: [5],
    edges: [],
    state: 'proposed',
    version: 1,
    ...overrides,
  }
}

function matrix(version: number, uncovered: string[]): SpecCoverageMatrixResponse {
  return {
    spec_id: 1,
    version,
    stories: [
      { story: 'CORE-S1-US1', claims: [{ ticket_id: 5, ticket_number: 5, outcome: 'Claimed.' }] },
      ...uncovered.map((story) => ({ story, claims: [] })),
    ],
  }
}

interface Options {
  proposals: TicketGraphRecord[]
  /** The coverage of each Spec content version, by version number. */
  coverage: Record<number, SpecCoverageMatrixResponse>
  /** The version the core resolves when a read names none. */
  operative: number
  refuseVersion?: number
}

function harness(options: Options) {
  const queries: Array<{ name: string; request: Record<string, unknown> }> = []
  const transport = {
    query: (name: string, request: Record<string, unknown>) => {
      queries.push({ name, request })
      switch (name) {
        case 'project.list':
          return Promise.resolve({ projects: [project] })
        case 'plan.list':
          return Promise.resolve({ plans: [] })
        case 'spec.list':
          return Promise.resolve({ specs })
        case 'ticket.list':
          return Promise.resolve({ tickets: [] })
        case 'ticket.graph.list':
          return Promise.resolve({
            proposals: request.spec_id === 1 ? options.proposals : [],
          })
        case 'spec.coverage.matrix': {
          const asked = (request.version as number | null) ?? options.operative
          if (options.refuseVersion === asked) {
            return Promise.reject({ code: 'not_found', message: `spec version ${asked}` })
          }
          const report = options.coverage[asked]
          return report
            ? Promise.resolve(report)
            : Promise.reject(new Error(`no coverage fixture for version ${asked}`))
        }
        default:
          return Promise.resolve({})
      }
    },
    command: () => Promise.resolve({}),
    subscribe: () => () => undefined,
    onConnectionChange: () => () => undefined,
  } as unknown as ShellTransport
  return { transport, queries }
}

const mounted: VueWrapper[] = []
afterEach(() => {
  for (const wrapper of mounted.splice(0)) wrapper.unmount()
})

async function mountPlanning(transport: ShellTransport, path = '/planning?project=4&spec=1') {
  await router.push(path)
  await router.isReady()
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

describe('graph coverage basis', () => {
  it('leaves a covered older proposal alone when the operative version is uncovered', async () => {
    const { transport, queries } = harness({
      operative: 2,
      proposals: [proposal({ id: 11, spec_version: 1 })],
      coverage: { 1: matrix(1, []), 2: matrix(2, ['CORE-S1-US2']) },
    })
    const wrapper = await mountPlanning(transport)

    expect(wrapper.get('[data-testid="coverage-version"]').text()).toBe('v2')
    expect(wrapper.find('[data-testid="graph-blocking-11"]').exists()).toBe(false)
    expect(wrapper.get('[data-testid="graph-covered-11"]').text()).toContain('v1')
    expect(
      queries.some(
        (entry) => entry.name === 'spec.coverage.matrix' && entry.request.version === 1,
      ),
      "the proposal's own version must be the basis read",
    ).toBe(true)
  })

  it('warns on an uncovered older proposal when the operative version is clear', async () => {
    const { transport } = harness({
      operative: 2,
      proposals: [proposal({ id: 11, spec_version: 1 })],
      coverage: { 1: matrix(1, ['CORE-S1-US4']), 2: matrix(2, []) },
    })
    const wrapper = await mountPlanning(transport)

    const warning = wrapper.get('[data-testid="graph-blocking-11"]')
    expect(warning.text()).toContain('CORE-S1-US4')
    expect(warning.text()).toContain('v1')
    expect(wrapper.find('[data-testid="graph-covered-11"]').exists()).toBe(false)
  })

  it('gives every retained proposal its own basis at once', async () => {
    const { transport } = harness({
      operative: 2,
      proposals: [
        proposal({ id: 11, spec_version: 1 }),
        proposal({ id: 12, spec_version: 2 }),
      ],
      coverage: { 1: matrix(1, ['CORE-S1-US4']), 2: matrix(2, []) },
    })
    const wrapper = await mountPlanning(transport)

    expect(wrapper.get('[data-testid="graph-blocking-11"]').text()).toContain('CORE-S1-US4')
    expect(wrapper.find('[data-testid="graph-blocking-12"]').exists()).toBe(false)
    expect(wrapper.get('[data-testid="graph-covered-12"]').text()).toContain('v2')
  })

  it('says so rather than implying coverage when a version cannot be read', async () => {
    const { transport } = harness({
      operative: 2,
      proposals: [proposal({ id: 11, spec_version: 1 })],
      coverage: { 2: matrix(2, []) },
      refuseVersion: 1,
    })
    const wrapper = await mountPlanning(transport)

    const failed = wrapper.get('[data-testid="graph-coverage-error-11"]')
    expect(failed.text()).toContain('v1')
    expect(wrapper.find('[data-testid="graph-covered-11"]').exists()).toBe(false)
    expect(wrapper.find('[data-testid="graph-blocking-11"]').exists()).toBe(false)
  })

  it('drops every proposal basis when the Spec on display changes', async () => {
    const { transport } = harness({
      operative: 2,
      proposals: [proposal({ id: 11, spec_version: 1 })],
      coverage: { 1: matrix(1, ['CORE-S1-US4']), 2: matrix(2, []) },
    })
    const wrapper = await mountPlanning(transport)

    expect(wrapper.get('[data-testid="graph-blocking-11"]').text()).toContain('CORE-S1-US4')

    await router.push('/planning?project=4&spec=9')
    await flushPromises()

    expect(wrapper.find('[data-testid="graph-blocking-11"]').exists()).toBe(false)
    expect(wrapper.find('[data-testid="graph-empty"]').exists()).toBe(true)
  })
})
