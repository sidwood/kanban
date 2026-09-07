// KAN-T30-AC1: the Done table lists completed work with the same
// filters and ordering as the board — terminal states stay off the
// surface, and the deterministic order the columns scan is the order
// the table scans.
import { flushPromises, mount } from '@vue/test-utils'
import { createPinia } from 'pinia'
import { afterEach, describe, expect, it, vi } from 'vitest'
import type {
  ProjectListResponse,
  SpecListResponse,
  TicketListResponse,
  TicketRecord,
} from '@kanban/contracts'
import router from '../router'
import { kanbanTransportKey } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import BoardView from './BoardView.vue'
import { orderCards } from './board-ordering'
import { columnForCard } from './board-layout'
import { loadBoardChoices, saveBoardChoices } from './board-layout.storage'

const project = {
  id: 1,
  code: 'KAN',
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
}

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

function harness(tickets: TicketRecord[]) {
  const query = vi.fn((name: string, request: unknown) => {
    if (name === 'project.list') {
      return Promise.resolve({ projects: [project] } satisfies ProjectListResponse)
    }
    if (name === 'lane.list') {
      return Promise.resolve({ lanes: [] })
    }
    if (name === 'run.list') {
      return Promise.resolve({ project_id: 1, runs: [] })
    }
    if (name === 'spec.list') {
      return Promise.resolve({ specs: [] } satisfies SpecListResponse)
    }
    if (name === 'ticket.readiness') {
      const { ticket_id } = request as { ticket_id: number }
      return Promise.resolve({
        blocked_by: [],
        ready: true,
        state: 'ready',
        ticket_id,
      })
    }
    return Promise.resolve({ tickets } satisfies TicketListResponse)
  })
  return {
    query,
    transport: {
      query,
      command: vi.fn(),
      subscribe: () => () => undefined,
      onConnectionChange: () => () => undefined,
    } as unknown as ShellTransport,
  }
}

async function mountedDoneTable(tickets: TicketRecord[]) {
  document.documentElement.classList.remove('dark')
  localStorage.clear()
  saveBoardChoices({ ...loadBoardChoices(), done: 'table' })
  const { transport } = harness(tickets)
  await router.push('/projects/1/board')
  const wrapper = mount(BoardView, {
    global: {
      plugins: [createPinia(), router],
      provide: { [kanbanTransportKey as symbol]: transport },
    },
  })
  await flushPromises()
  return wrapper
}

function doneTableIds(wrapper: Awaited<ReturnType<typeof mountedDoneTable>>): number[] {
  return wrapper
    .findAll('[data-testid="done-table"] [data-testid^="open-ticket-"]')
    .map((row) => Number((row.attributes('data-testid') ?? '').replace('open-ticket-', '')))
}

function doneColumnIds(wrapper: Awaited<ReturnType<typeof mountedDoneTable>>): number[] {
  return wrapper
    .findAll('[data-testid="kanban-column-done"] [data-testid^="kanban-card-"]')
    .map((card) => Number((card.attributes('data-testid') ?? '').replace('kanban-card-', '')))
}

afterEach(() => {
  document.documentElement.classList.remove('dark')
  localStorage.clear()
})

describe('done table', () => {
  it('lists only on-board done tickets, never terminal states', async () => {
    const tickets = [
      ticket({ id: 10, number: 15, state: 'done', title: 'Landed the export path' }),
      ticket({ id: 12, number: 16, state: 'cancelled' }),
      ticket({ id: 13, number: 17, state: 'superseded' }),
      ticket({ id: 14, number: 18, state: 'done', title: 'Closed the loop' }),
    ]
    const wrapper = await mountedDoneTable(tickets)

    expect(doneTableIds(wrapper)).toEqual([10, 14])
    expect(wrapper.find('[data-testid="done-count"]').text()).toBe('2')
  })

  it('orders done rows the same way the board column would', async () => {
    const layouts = loadBoardChoices().layouts
    const tickets = [
      ticket({ id: 10, number: 30, state: 'done', priority: 'low', title: 'Low priority' }),
      ticket({ id: 11, number: 5, state: 'done', priority: 'urgent', title: 'Urgent first' }),
      ticket({ id: 12, number: 12, state: 'done', priority: 'normal', title: 'Middle tie' }),
      ticket({ id: 13, number: 8, state: 'done', priority: 'normal', title: 'Earlier number' }),
    ]
    const expected = orderCards(tickets)
      .filter((entry) => columnForCard(entry.state, layouts) === 'done')
      .map((entry) => entry.id)

    const wrapper = await mountedDoneTable(tickets.reverse())
    expect(doneTableIds(wrapper)).toEqual(expected)
  })

  it('matches the done column before demotion and the table after', async () => {
    const tickets = [
      ticket({ id: 10, number: 15, state: 'done', priority: 'high', title: 'First done' }),
      ticket({ id: 11, number: 20, state: 'done', priority: 'normal', title: 'Second done' }),
    ]
    document.documentElement.classList.remove('dark')
    localStorage.clear()
    saveBoardChoices({ ...loadBoardChoices(), done: 'column' })
    const { transport } = harness(tickets)
    await router.push('/projects/1/board')
    const wrapper = mount(BoardView, {
      global: {
        plugins: [createPinia(), router],
        provide: { [kanbanTransportKey as symbol]: transport },
      },
    })
    await flushPromises()

    expect(doneColumnIds(wrapper)).toEqual([10, 11])

    await wrapper.find('[data-testid="move-done-below-board"]').trigger('click')
    await flushPromises()

    expect(wrapper.find('[data-testid="kanban-column-done"]').exists()).toBe(false)
    expect(doneTableIds(wrapper)).toEqual([10, 11])
  })
})
