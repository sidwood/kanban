import { flushPromises, mount } from '@vue/test-utils'
import { createPinia } from 'pinia'
import { describe, expect, it, vi } from 'vitest'
import type { BoardGlobalResponse, TicketRecord } from '@kanban/contracts'
import router from '../router'
import { kanbanTransportKey } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import GlobalBoardView from './GlobalBoardView.vue'

const ticket = (overrides: Partial<TicketRecord> = {}): TicketRecord => ({
  id: 7,
  project_id: 1,
  number: 12,
  kind: 'task',
  priority: 'normal',
  state: 'ready',
  spec_id: null,
  title: 'Archive the old exports',
  slice: null,
  criteria: [],
  bug: null,
  subtype: 'operational',
  mode: 'human',
  completion: ['The old exports are archived.'],
  scheduled_for: null,
  due: null,
  profile: null,
  version: 3,
  ...overrides,
})

// The projection the core returns: grouped, ordered, with the filter
// values every axis offers.
function boardResponse(): BoardGlobalResponse {
  return {
    cards: [
      {
        group: 'backlog',
        project_code: 'CORE',
        spec_number: null,
        lane_id: 4,
        ticket: ticket(),
      },
      {
        group: 'current',
        project_code: 'EDGE',
        spec_number: 9,
        lane_id: null,
        ticket: ticket({
          id: 8,
          project_id: 2,
          number: 3,
          kind: 'implementation',
          state: 'active',
          title: null,
          slice: 'Serve the lifecycle command surface',
        }),
      },
      {
        group: 'done',
        project_code: 'CORE',
        spec_number: null,
        lane_id: null,
        ticket: ticket({ id: 9, number: 15, state: 'done', version: 9 }),
      },
    ],
    options: {
      initiatives: [{ id: 1, label: 'Personal tooling' }],
      projects: [
        { id: 1, label: 'CORE — Control plane' },
        { id: 2, label: 'EDGE — Edge tooling' },
      ],
      plans: [{ id: 3, label: 'CORE-P1' }],
      specs: [],
      lanes: [],
      profiles: ['standard'],
      attention: ['blocker', 'stale_run'],
    },
  }
}

// The views a board mounts over: the generated global default, and a
// named review perspective a switch test restores.
const viewFixture = {
  id: 8,
  name: 'Review queue',
  scope: 'global',
  filter: { states: ['in_review'] },
  expanded_groups: ['backlog', 'staged'],
  hidden_columns: ['draft', 'done'],
  mode: 'register',
  done_placement: 'table',
  sorting: 'readiness',
  is_default: false,
  version: 4,
}

function viewList() {
  return {
    views: [
      {
        id: 1,
        name: 'All work',
        scope: 'global',
        filter: {},
        expanded_groups: [],
        hidden_columns: ['draft'],
        mode: 'board',
        done_placement: 'column',
        sorting: 'priority',
        is_default: true,
        version: 1,
      },
      viewFixture,
    ],
  }
}

function harness(answer: (request: unknown) => Promise<BoardGlobalResponse>) {
  const query = vi.fn((name: string, request: unknown) => {
    if (name === 'board.global') return answer(request)
    if (name === 'view.list') return Promise.resolve(viewList())
    throw new Error(`unexpected query ${name}`)
  })
  const command = vi.fn((name: string, request: unknown) => {
    if (name === 'view.create') {
      const body = request as { name: string } & Record<string, unknown>
      return Promise.resolve({ ...viewFixture, id: 21, ...body, version: 1 })
    }
    if (name === 'view.update') {
      const body = request as { view_id: number } & Record<string, unknown>
      const standing =
        [viewList().views[0], viewFixture].find((view) => view.id === body.view_id) ?? viewFixture
      return Promise.resolve({
        ...standing,
        expanded_groups: (body.expanded_groups as typeof standing.expanded_groups) ?? standing.expanded_groups,
        hidden_columns: (body.hidden_columns as typeof standing.hidden_columns) ?? standing.hidden_columns,
        mode: (body.mode as typeof standing.mode) ?? standing.mode,
        done_placement: (body.done_placement as typeof standing.done_placement) ?? standing.done_placement,
        sorting: (body.sorting as typeof standing.sorting) ?? standing.sorting,
        version: standing.version + 1,
      })
    }
    return Promise.resolve({})
  })
  const transport = {
    query,
    command,
    subscribe: () => () => undefined,
  } as unknown as ShellTransport
  return { transport, query, command }
}

async function mountBoard(transport: ShellTransport) {
  await router.push('/board')
  const wrapper = mount(GlobalBoardView, {
    global: {
      plugins: [createPinia(), router],
      provide: { [kanbanTransportKey as symbol]: transport },
    },
  })
  await flushPromises()
  return wrapper
}

describe('global board view', () => {
  it('renders the fixed groups with the projection\'s cards in place', async () => {
    const { transport } = harness(() => Promise.resolve(boardResponse()))
    const wrapper = await mountBoard(transport)

    const headings = wrapper
      .findAll('[data-testid="global-board-group"]')
      .map((group) => group.attributes('data-group'))
    expect(headings).toEqual(['draft', 'backlog', 'current', 'review', 'staged', 'done'])

    const backlog = wrapper.get('[data-testid="global-board-group"][data-group="backlog"]')
    expect(backlog.text()).toContain('CORE-T12')
    expect(backlog.text()).toContain('Archive the old exports')
    const current = wrapper.get('[data-testid="global-board-group"][data-group="current"]')
    expect(current.text()).toContain('EDGE-T3')
    expect(current.text()).toContain('Serve the lifecycle command surface')
  })

  it('offers every axis and re-queries on one toggled value', async () => {
    let filter: unknown
    const { transport, query } = harness((request) => {
      filter = (request as { filter: unknown }).filter
      return Promise.resolve(boardResponse())
    })
    const wrapper = await mountBoard(transport)

    // The reference axes offer what the core listed; the closed
    // vocabularies offer their words.
    const panel = wrapper.get('[data-testid="global-board-filters"]')
    expect(panel.text()).toContain('Personal tooling')
    expect(panel.text()).toContain('CORE — Control plane')
    expect(panel.text()).toContain('CORE-P1')
    expect(panel.text()).toContain('standard')
    expect(panel.text()).toContain('Stale run')

    const projectOption = panel.find('input[value="projects:2"]')
    expect(projectOption.exists()).toBe(true)
    await projectOption.setValue(true)
    await flushPromises()

    expect(filter).toEqual({ projects: [2] })
    // The views loaded once on mount; the projection re-queried for
    // the toggled axis alone.
    expect(query.mock.calls.filter(([name]) => name === 'board.global')).toHaveLength(2)
    expect(query.mock.calls.filter(([name]) => name === 'view.list')).toHaveLength(1)
  })

  it('switches to a saved view and queries under the filter it owns', async () => {
    let filter: unknown
    const { transport } = harness((request) => {
      filter = (request as { filter: unknown }).filter
      return Promise.resolve(boardResponse())
    })
    const wrapper = await mountBoard(transport)
    expect(filter).toEqual({})

    const select = wrapper.get('[data-testid="global-view-select"]')
    await select.setValue('8')
    await flushPromises()

    // The view's owned filter travels to the core whole.
    expect(filter).toEqual({ states: ['in_review'] })
    expect((select.element as HTMLSelectElement).selectedOptions[0]?.text).toBe('Review queue')

    // Switching back restores the default's empty filter exactly.
    await select.setValue('1')
    await flushPromises()
    expect(filter).toEqual({})
  })

  it('names and keeps the perspective the board holds', async () => {
    const { transport, command } = harness(() => Promise.resolve(boardResponse()))
    const wrapper = await mountBoard(transport)

    await wrapper.get('[data-testid="save-view-name"]').setValue('Deep work')
    await wrapper.get('[data-testid="save-view"]').trigger('click')
    await flushPromises()

    const create = command.mock.calls.find(([name]) => name === 'view.create')
    expect(create?.[1]).toMatchObject({
      scope: 'global',
      name: 'Deep work',
      mode: 'board',
      done_placement: 'column',
      sorting: 'priority',
    })
    // The kept perspective becomes the one the board rests on.
    const select = wrapper.get('[data-testid="global-view-select"]')
    expect((select.element as HTMLSelectElement).value).toBe('21')
  })

  it('clears every axis from the filter bar', async () => {
    let filter: unknown
    const { transport } = harness((request) => {
      filter = (request as { filter: unknown }).filter
      return Promise.resolve(boardResponse())
    })
    const wrapper = await mountBoard(transport)
    await wrapper.find('input[value="kinds:task"]').setValue(true)
    await flushPromises()
    expect(filter).toEqual({ kinds: ['task'] })

    await wrapper.get('[data-testid="global-board-clear"]').trigger('click')
    await flushPromises()

    expect(filter).toEqual({})
  })

  it('shows the empty state when no work matches', async () => {
    const { transport } = harness(() =>
      Promise.resolve({ cards: [], options: boardResponse().options }),
    )
    const wrapper = await mountBoard(transport)

    expect(wrapper.text()).toContain('No work matches')
  })

  it('reports a failed load', async () => {
    const { transport } = harness(() =>
      Promise.reject({ code: 'unavailable', message: 'the core is offline' }),
    )
    const wrapper = await mountBoard(transport)

    expect(wrapper.text()).toContain('the core is offline')
  })
})
