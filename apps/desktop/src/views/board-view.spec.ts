import { mount, flushPromises } from '@vue/test-utils'
import type { DOMWrapper } from '@vue/test-utils'
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

const otherProject = { ...project, id: 2, code: 'SFC', name: 'Surface fleet' }

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

// The board's working set: one card per region the board must place.
function boardTickets(): TicketRecord[] {
  return [
    ticket(),
    ticket({
      id: 8,
      number: 13,
      kind: 'implementation',
      state: 'active',
      title: null,
      slice: 'Serve the lifecycle command surface',
      spec_id: 4,
      subtype: null,
      mode: null,
      completion: [],
      version: 5,
    }),
    ticket({
      id: 9,
      number: 14,
      kind: 'bug',
      state: 'approved',
      title: 'Clone guard misses a dirty tree',
      subtype: null,
      mode: null,
      completion: [],
      version: 2,
    }),
    ticket({ id: 10, number: 15, state: 'done', version: 9 }),
    ticket({ id: 12, number: 16, state: 'cancelled' }),
    ticket({ id: 13, number: 17, state: 'superseded' }),
  ]
}

// A ticket.list answer the test settles by hand: the load that is
// still on the wire when the board changes Project.
function deferred<T>() {
  let settle!: (value: T) => void
  const promise = new Promise<T>((resolve) => {
    settle = resolve
  })
  return { promise, settle }
}

// Answers the queries a board load spends. ticket.list for a Project
// named in pendingProjects stays on the wire for good: the board it
// asked for never settles while the test is watching.
function harness(tickets: TicketRecord[], pendingProjects: readonly number[] = []) {
  const query = vi.fn((name: string, request: unknown) => {
    if (name === 'project.list') {
      return Promise.resolve({
        projects: [project, otherProject],
      } satisfies ProjectListResponse)
    }
    if (name === 'lane.list') {
      return Promise.resolve({ lanes: [] } satisfies { lanes: [] })
    }
    // The board loads the Project's Specs beside its Tickets
    // (KAN-T126); the numbers its cards render arrive with the tests
    // that assert them.
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
    const { project_id } = request as { project_id: number }
    if (pendingProjects.includes(project_id)) {
      return new Promise<TicketListResponse>(() => undefined)
    }
    return Promise.resolve({ tickets } satisfies TicketListResponse)
  })
  const command = vi.fn()
  const transport = {
    query,
    command,
    subscribe: () => () => undefined,
    onConnectionChange: () => () => undefined,
  } as unknown as ShellTransport
  return { transport, query, command }
}

// Mount the board on one Project's route, flushing the load that
// arrival spends. A shared pinia carries one store across mounts,
// the way the running application does across views.
async function mountBoard(
  transport: ShellTransport,
  projectId: number,
  pinia = createPinia(),
) {
  document.documentElement.classList.remove('dark')
  localStorage.clear()
  await router.push(`/projects/${projectId}/board`)
  const wrapper = mount(BoardView, {
    global: {
      plugins: [pinia, router],
      provide: { [kanbanTransportKey as symbol]: transport },
    },
  })
  await flushPromises()
  return wrapper
}

async function mounted(tickets: TicketRecord[], pendingProjects: readonly number[] = []) {
  const { transport, query, command } = harness(tickets, pendingProjects)
  const wrapper = await mountBoard(transport, 1)
  return { wrapper, transport, query, command }
}

afterEach(() => {
  document.documentElement.classList.remove('dark')
  localStorage.clear()
})

// Drag one card onto one column: the drag the interaction language
// spends, compressed to the two events the board handles.
function dragCard(card: DOMWrapper<Element>, column: DOMWrapper<Element>): Promise<void> {
  const dataTransfer = {
    effectAllowed: '',
    setData: () => undefined,
  }
  return card
    .trigger('dragstart', { dataTransfer })
    .then(() => column.trigger('drop', { preventDefault: () => undefined, dataTransfer }))
}

describe('BoardView', () => {
  it('renders the Project\'s real Tickets through the generated client', async () => {
    const { wrapper, query } = await mounted(boardTickets())

    expect(query).toHaveBeenCalledWith('ticket.list', { project_id: 1 })
    expect(wrapper.find('[data-testid="kanban-column-backlog"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="kanban-card-7"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="card-number-7"]').text()).toBe('KAN-T12')
    expect(wrapper.find('[data-testid="open-ticket-8"]').text()).toBe(
      'Serve the lifecycle command surface',
    )
  })

  it('places each card in the group its state maps to', async () => {
    const { wrapper } = await mounted(boardTickets())

    expect(wrapper.find('[data-testid="kanban-column-ready"]').exists()).toBe(false)
    expect(
      wrapper.find('[data-testid="kanban-column-backlog"] [data-testid="kanban-card-7"]')
        .exists(),
    ).toBe(true)
    expect(
      wrapper.find('[data-testid="kanban-column-current"] [data-testid="kanban-card-8"]')
        .exists(),
    ).toBe(true)
    expect(
      wrapper.find('[data-testid="kanban-column-staged"] [data-testid="kanban-card-9"]')
        .exists(),
    ).toBe(true)
    expect(
      wrapper.find('[data-testid="kanban-column-done"] [data-testid="kanban-card-10"]')
        .exists(),
    ).toBe(true)
  })

  it('never renders terminal states on the board', async () => {
    const { wrapper } = await mounted(boardTickets())

    expect(wrapper.find('[data-testid="kanban-card-12"]').exists()).toBe(false)
    expect(wrapper.find('[data-testid="kanban-card-13"]').exists()).toBe(false)
  })

  it('names the state a multi-state column holds on the card', async () => {
    const { wrapper } = await mounted(boardTickets())

    expect(wrapper.find('[data-testid="card-status-7"]').text()).toBe('Ready')
    // Current holds one state, so its card needs no badge.
    expect(wrapper.find('[data-testid="card-status-8"]').exists()).toBe(false)
  })

  it('drags only Task Tickets, and a drop asks the core for the column state', async () => {
    const { wrapper, command } = await mounted(boardTickets())

    expect(wrapper.find('[data-testid="kanban-card-7"]').attributes('draggable')).toBe(
      'true',
    )
    expect(wrapper.find('[data-testid="kanban-card-8"]').attributes('draggable')).toBe(
      'false',
    )
    expect(wrapper.find('[data-testid="kanban-card-9"]').attributes('draggable')).toBe(
      'false',
    )

    await dragCard(
      wrapper.find('[data-testid="kanban-card-7"]'),
      wrapper.find('[data-testid="kanban-column-current"]'),
    )
    await flushPromises()

    expect(command).toHaveBeenCalledWith(
      'ticket.transition',
      expect.objectContaining({
        ticket_id: 7,
        to: 'active',
        mutation: expect.objectContaining({ optimistic_version: 3 }),
      }),
    )
  })

  it('reports the core\'s refusal of an agent-owned drag', async () => {
    const { wrapper, command } = await mounted(boardTickets())
    command.mockRejectedValue({
      code: 'invalid_request',
      message: 'bug transitions are agent-owned; a human may drag only Task Tickets',
    })

    await dragCard(
      wrapper.find('[data-testid="kanban-card-7"]'),
      wrapper.find('[data-testid="kanban-column-current"]'),
    )
    await flushPromises()

    expect(wrapper.find('[data-testid="board-error"]').text()).toContain(
      'bug transitions are agent-owned',
    )
  })

  it('keeps Draft off the board until cards sit in it or the operator asks', async () => {
    const noDrafts = await mounted(boardTickets().filter((entry) => entry.state !== 'draft'))
    expect(noDrafts.wrapper.find('[data-testid="kanban-column-draft"]').exists()).toBe(
      false,
    )
    expect(noDrafts.wrapper.find('[data-testid="draft-count"]').text()).toBe('0')
    await noDrafts.wrapper.find('[data-testid="toggle-draft"]').trigger('click')
    expect(noDrafts.wrapper.find('[data-testid="kanban-column-draft"]').exists()).toBe(
      true,
    )

    const withDraft = await mounted([
      ...boardTickets(),
      ticket({ id: 11, number: 18, state: 'draft' }),
    ])
    expect(withDraft.wrapper.find('[data-testid="kanban-column-draft"]').exists()).toBe(
      true,
    )
    // Cards in Draft hold the column open, so the toggle offers to hide it.
    expect(withDraft.wrapper.find('[data-testid="toggle-draft"]').text()).toContain(
      'Hide Draft',
    )
  })

  it('opens an axis into its states and collapses it back', async () => {
    const { wrapper } = await mounted(boardTickets())

    await wrapper
      .find('[data-testid="layout-axis-backlog-expanded"]')
      .trigger('click')

    expect(wrapper.find('[data-testid="kanban-column-parked"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="kanban-column-scheduled"]').exists()).toBe(true)
    expect(
      wrapper.find('[data-testid="kanban-column-ready"] [data-testid="kanban-card-7"]')
        .exists(),
    ).toBe(true)
    expect(wrapper.find('[data-testid="kanban-group-backlog"]').attributes('data-grouped')).toBe(
      'true',
    )

    await wrapper.find('[data-testid="layout-axis-backlog-collapsed"]').trigger('click')
    expect(wrapper.find('[data-testid="kanban-column-parked"]').exists()).toBe(false)
  })

  it('moves a register row by naming the column it goes to', async () => {
    const { wrapper, command } = await mounted(boardTickets())

    await wrapper.find('[data-testid="board-presentation-register"]').trigger('click')

    expect(wrapper.find('[data-testid="board-register"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="register-column-backlog"]').exists()).toBe(true)

    await wrapper.find('[data-testid="move-7"]').setValue('current')
    await flushPromises()

    expect(command).toHaveBeenCalledWith(
      'ticket.transition',
      expect.objectContaining({ ticket_id: 7, to: 'active' }),
    )
    // The select is an action, not a state: it returns to its prompt.
    expect((wrapper.find('[data-testid="move-7"]').element as HTMLSelectElement).value).toBe(
      '',
    )
  })

  it('demotes Done to the table below the board and brings it back', async () => {
    const { wrapper } = await mounted(boardTickets())

    await wrapper.find('[data-testid="move-done-below-board"]').trigger('click')

    expect(wrapper.find('[data-testid="kanban-column-done"]').exists()).toBe(false)
    expect(wrapper.find('[data-testid="done-table"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="done-count"]').text()).toBe('1')

    await wrapper.find('[data-testid="bring-done-back-to-board"]').trigger('click')
    expect(wrapper.find('[data-testid="kanban-column-done"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="done-table"]').exists()).toBe(false)
  })

  it('opens a card in the detail drawer and closes it', async () => {
    const { wrapper } = await mounted(boardTickets())

    await wrapper.find('[data-testid="open-ticket-7"]').trigger('click')
    await flushPromises()

    const heading = document.querySelector('[role="dialog"] h2')
    expect(heading?.textContent).toContain('KAN-T12')
    expect(heading?.textContent).toContain('Archive the old exports')
    expect(document.querySelector('[data-testid="drawer-state"]')?.textContent).toBe(
      'Ready',
    )

    ;(document.querySelector('[aria-label="Close panel"]') as HTMLElement).click()
    await flushPromises()
    expect(document.querySelector('[role="dialog"]')).toBeNull()
  })

  it('swaps the class-based theme from the board header', async () => {
    const { wrapper } = await mounted(boardTickets())

    await wrapper.find('[data-testid="theme-dark"]').trigger('click')
    expect(document.documentElement.classList.contains('dark')).toBe(true)

    await wrapper.find('[data-testid="theme-light"]').trigger('click')
    expect(document.documentElement.classList.contains('dark')).toBe(false)
  })

  it('pins the Surface presentation on the board surface', async () => {
    const { wrapper } = await mounted(boardTickets())

    const board = wrapper.find('[data-testid="kanban-board"]')
    expect(board.classes()).toContain('overflow-x-auto')
    expect(board.classes()).toContain('md:flex-row')
    expect(board.attributes('data-backlog-layout')).toBe('collapsed')
    expect(board.attributes('data-completion-layout')).toBe('collapsed')

    const column = wrapper.find('[data-testid="kanban-column-backlog"]')
    expect(column.classes()).toContain('rounded-panel')
    expect(column.classes()).toContain('bg-surface/80')

    const card = wrapper.find('[data-testid="kanban-card-7"]')
    expect(card.classes()).toContain('shadow-panel')
    expect(card.classes()).toContain('rounded-control')

    const heading = wrapper.find('#kanban-heading-backlog')
    expect(heading.classes()).toContain('font-display')
    expect(wrapper.find('[data-testid="board-error"]').exists()).toBe(false)
  })

  it('shows the loading columns while the board arrives', async () => {
    const { transport } = harness(boardTickets())
    ;(transport.query as ReturnType<typeof vi.fn>).mockImplementation(
      () => new Promise(() => undefined),
    )
    document.documentElement.classList.remove('dark')
    localStorage.clear()
    router.push('/projects/1/board')
    await router.isReady()
    const wrapper = mount(BoardView, {
      global: {
        plugins: [createPinia(), router],
        provide: { [kanbanTransportKey as symbol]: transport },
      },
    })
    await flushPromises()

    expect(wrapper.find('[data-testid="board-loading"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="kanban-board"]').exists()).toBe(false)
  })

  it('says which Project it cannot board', async () => {
    document.documentElement.classList.remove('dark')
    localStorage.clear()
    const query = vi.fn((name: string) => {
      if (name === 'project.list') {
        return Promise.resolve({ projects: [] } satisfies ProjectListResponse)
      }
      return Promise.resolve({ tickets: [] } satisfies TicketListResponse)
    })
    const transport = {
      query,
      command: vi.fn(),
      subscribe: () => () => undefined,
      onConnectionChange: () => () => undefined,
    } as unknown as ShellTransport
    router.push('/projects/1/board')
    await router.isReady()
    const wrapper = mount(BoardView, {
      global: {
        plugins: [createPinia(), router],
        provide: { [kanbanTransportKey as symbol]: transport },
      },
    })
    await flushPromises()

    expect(wrapper.find('[data-testid="board-project-missing"]').exists()).toBe(true)
  })

  it('clears the previous Project\'s cards while the next load is on the wire', async () => {
    const { wrapper, query } = await mounted(boardTickets())
    expect(wrapper.find('[data-testid="kanban-card-7"]').exists()).toBe(true)

    // The Project register itself answers slowly: the board's own
    // load has not even begun when the Project changes.
    ;(query as ReturnType<typeof vi.fn>).mockImplementation(
      () => new Promise(() => undefined),
    )
    router.push('/projects/2/board')
    await flushPromises()

    // Cards, counts, everything of the Project left behind goes
    // before the next load settles (KAN-T125-AC1).
    expect(wrapper.find('[data-testid="kanban-card-7"]').exists()).toBe(false)
    expect(wrapper.find('[data-testid="board-loading"]').exists()).toBe(true)
  })

  it('takes the drawer with it when the Project changes', async () => {
    const { wrapper } = await mounted(boardTickets(), [2])

    await wrapper.find('[data-testid="open-ticket-7"]').trigger('click')
    await flushPromises()
    expect(document.querySelector('[role="dialog"]')).not.toBeNull()

    router.push('/projects/2/board')
    await flushPromises()
    expect(document.querySelector('[role="dialog"]')).toBeNull()

    // Returning to the first Project reopens nothing on its own.
    router.push('/projects/1/board')
    await flushPromises()
    expect(document.querySelector('[role="dialog"]')).toBeNull()
  })

  it('keeps no cards for a Project that is not registered', async () => {
    const { wrapper } = await mounted(boardTickets())
    expect(wrapper.find('[data-testid="kanban-card-7"]').exists()).toBe(true)

    router.push('/projects/3/board')
    await flushPromises()

    expect(wrapper.find('[data-testid="board-project-missing"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="kanban-card-7"]').exists()).toBe(false)
  })

  it('refuses a drop of a card captured on another Project\'s board', async () => {
    const { wrapper, query, command } = await mounted(boardTickets())
    const dataTransfer = { effectAllowed: '', setData: () => undefined }

    await wrapper
      .find('[data-testid="kanban-card-7"]')
      .trigger('dragstart', { dataTransfer })

    // The next Project's board fails to arrive at all.
    ;(query as ReturnType<typeof vi.fn>).mockImplementation((name: string) => {
      if (name === 'project.list') {
        return Promise.resolve({
          projects: [project, otherProject],
        } satisfies ProjectListResponse)
      }
      return Promise.reject({ code: 'unavailable', message: 'the core is offline' })
    })
    router.push('/projects/2/board')
    await flushPromises()

    await wrapper.find('[data-testid="kanban-column-current"]').trigger('drop', {
      preventDefault: () => undefined,
      dataTransfer,
    })
    await flushPromises()

    // The drag was captured under the previous Project's heading; the
    // board it landed on never sends it (KAN-T125-AC3).
    expect(command).not.toHaveBeenCalled()
    expect(wrapper.find('[data-testid="board-error"]').text()).toContain(
      'does not hold Ticket 7',
    )
  })

  it('keeps the last Project when a slower earlier load settles late', async () => {
    const slow = deferred<TicketListResponse>()
    const query = vi.fn((name: string, request: unknown) => {
      if (name === 'project.list') {
        return Promise.resolve({
          projects: [project, otherProject],
        } satisfies ProjectListResponse)
      }
      if (name === 'lane.list') {
        return Promise.resolve({ lanes: [] } satisfies { lanes: [] })
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
      const { project_id } = request as { project_id: number }
      if (project_id === 2) return slow.promise
      return Promise.resolve({ tickets: boardTickets() } satisfies TicketListResponse)
    })
    const transport = {
      query,
      command: vi.fn(),
      subscribe: () => () => undefined,
      onConnectionChange: () => () => undefined,
    } as unknown as ShellTransport
    const wrapper = await mountBoard(transport, 1)
    expect(wrapper.find('[data-testid="kanban-card-7"]').exists()).toBe(true)

    router.push('/projects/2/board')
    await flushPromises()
    router.push('/projects/1/board')
    await flushPromises()
    expect(wrapper.find('[data-testid="kanban-card-7"]').exists()).toBe(true)

    // The slower board for the Project in between settles last and
    // renders none of it (KAN-T125-AC2).
    slow.settle({ tickets: [ticket({ id: 21, project_id: 2, number: 22 })] })
    await flushPromises()
    expect(wrapper.find('[data-testid="kanban-card-7"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="kanban-card-21"]').exists()).toBe(false)
  })

  it('mounts empty over the board a previous view kept', async () => {
    const pinia = createPinia()
    const first = harness(boardTickets())
    const firstWrapper = await mountBoard(first.transport, 1, pinia)
    expect(firstWrapper.find('[data-testid="kanban-card-7"]').exists()).toBe(true)
    firstWrapper.unmount()

    // The register answers slowly, so the new board's own load has
    // not even begun.
    const slow = {
      query: () => new Promise(() => undefined),
      command: vi.fn(),
      subscribe: () => () => undefined,
      onConnectionChange: () => () => undefined,
    } as unknown as ShellTransport
    const wrapper = await mountBoard(slow, 2, pinia)

    // A fresh mount over a store still holding another Project's
    // board renders none of it (KAN-T125-AC1).
    expect(wrapper.find('[data-testid="kanban-card-7"]').exists()).toBe(false)
    expect(wrapper.find('[data-testid="board-loading"]').exists()).toBe(true)
  })
})
