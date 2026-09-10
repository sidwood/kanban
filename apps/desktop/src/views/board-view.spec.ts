import { flushPromises, mount } from '@vue/test-utils'
import type { DOMWrapper, VueWrapper } from '@vue/test-utils'
import { createPinia } from 'pinia'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import type { BoardGlobalResponse, TicketRecord } from '@kanban/contracts'
import router from '../router'
import { kanbanTransportKey } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import { useShellStore } from '../stores/shell'
import { boardOptions, coreProject, edgeProject, harness, projection, ticket } from '../test/shell-harness'
import type { HarnessOptions } from '../test/shell-harness'
import { COLLAPSED_ROW_HEIGHT_PX } from './board-layout'
import BoardView from './BoardView.vue'

// The board's working set: one card per region the board must place,
// across two Projects.
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
    ticket({ id: 11, number: 3, project_id: 2, state: 'in_review', title: 'Edge review', version: 1 }),
  ]
}

// Every mount is taken down again, so one test's board never keeps
// reacting to the shared router under the next test's feet.
const mountedBoards: VueWrapper[] = []

async function mountBoard(transport: ShellTransport, path = '/projects/1/board', pinia = createPinia()) {
  await router.push(path)
  await router.isReady()
  const wrapper = mount(BoardView, {
    attachTo: document.body,
    global: {
      plugins: [pinia, router],
      provide: { [kanbanTransportKey as symbol]: transport },
    },
  })
  mountedBoards.push(wrapper)
  await flushPromises()
  return wrapper
}

async function mounted(options: HarnessOptions = {}, path = '/projects/1/board') {
  const shell = harness({ tickets: boardTickets(), ...options })
  const wrapper = await mountBoard(shell.transport, path)
  return { wrapper, ...shell }
}

beforeEach(() => {
  localStorage.clear()
  document.documentElement.classList.remove('dark')
})

afterEach(() => {
  for (const wrapper of mountedBoards.splice(0)) wrapper.unmount()
  document.body.innerHTML = ''
})

// Drag one card onto one column: the drag the interaction language
// spends, compressed to the three events the board handles. A drop
// only reaches a surface whose dragover was cancelled, so the helper
// insists on that the way a browser would.
function dragEvent(type: string, dataTransfer: object): Event {
  const event = new Event(type, { bubbles: true, cancelable: true })
  Object.defineProperty(event, 'dataTransfer', { value: dataTransfer })
  return event
}

type Surface = Pick<DOMWrapper<Element>, 'element'>

async function dragCard(card: Surface, column: Surface): Promise<void> {
  const dataTransfer = { effectAllowed: '', dropEffect: '', setData: () => undefined }
  card.element.dispatchEvent(dragEvent('dragstart', dataTransfer))
  await flushPromises()
  const over = dragEvent('dragover', dataTransfer)
  column.element.dispatchEvent(over)
  expect(over.defaultPrevented, 'the surface accepts the drop event').toBe(true)
  column.element.dispatchEvent(dragEvent('drop', dataTransfer))
  await flushPromises()
}

function boardCalls(query: ReturnType<typeof harness>['query']): unknown[] {
  return query.mock.calls.filter(([name]) => name === 'board.global').map(([, request]) => request)
}

describe('the board', () => {
  it('renders one Project\'s projection in the fixed groups, Draft hidden while empty', async () => {
    const { wrapper, query } = await mounted()

    expect(boardCalls(query)[0]).toEqual({ filter: { projects: [1] } })
    expect(wrapper.get('[data-testid="board-title"]').text()).toBe('Control plane')
    const groups = wrapper
      .findAll('[data-testid^="kanban-group-"]')
      .map((group) => group.attributes('data-testid'))
    expect(groups).toEqual([
      'kanban-group-backlog',
      'kanban-group-current',
      'kanban-group-review',
      'kanban-group-completion',
      'kanban-group-done',
    ])
    expect(wrapper.find('[data-testid="kanban-column-backlog"] [data-testid="kanban-card-7"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="kanban-column-current"] [data-testid="kanban-card-8"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="kanban-column-staged"] [data-testid="kanban-card-9"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="kanban-column-done"] [data-testid="kanban-card-10"]').exists()).toBe(true)
    // Another Project's card never reaches this Project's board.
    expect(wrapper.find('[data-testid="kanban-card-11"]').exists()).toBe(false)
    expect(wrapper.get('[data-testid="card-number-7"]').text()).toBe('CORE-T12')
    expect(wrapper.get('[data-testid="open-ticket-8"]').text()).toBe('Serve the lifecycle command surface')
    expect(wrapper.get('[data-testid="card-status-7"]').text()).toBe('Ready')
    expect(wrapper.find('[data-testid="card-status-8"]').exists()).toBe(false)
    expect(wrapper.get('[data-testid="board-count"]').text()).toContain('4 of 4')
  })

  it('renders every Project\'s work under the All projects scope', async () => {
    const { wrapper, query } = await mounted({}, '/board')

    expect(boardCalls(query)[0]).toEqual({ filter: {} })
    expect(wrapper.get('[data-testid="board-title"]').text()).toBe('All projects')
    expect(wrapper.find('[data-testid="kanban-column-review"] [data-testid="kanban-card-11"]').exists()).toBe(true)
    expect(wrapper.get('[data-testid="card-number-11"]').text()).toBe('EDGE-T3')
    expect(wrapper.get('[data-testid="card-project-11"]').text()).toContain('EDGE')
    expect(wrapper.get('[data-testid="board-count"]').text()).toContain('5 of 5')
  })

  it('shows the six groups when Draft has cards, and reveals Draft on its own', async () => {
    const { wrapper } = await mounted({
      tickets: [...boardTickets(), ticket({ id: 12, number: 16, state: 'draft' })],
    })

    expect(wrapper.find('[data-testid="kanban-column-draft"] [data-testid="kanban-card-12"]').exists()).toBe(true)
    // The record still says auto; a populated auto Draft cannot be
    // hidden, and the control says so rather than pretending.
    const toggle = wrapper.get('[data-testid="toggle-draft"]')
    expect(toggle.text()).toContain('Draft (auto)')
    expect(toggle.attributes('disabled')).toBeDefined()
    expect(wrapper.find('[data-testid="view-drift"]').exists()).toBe(false)
  })

  it('refuses a drop on Draft without sending anything', async () => {
    const { wrapper, command } = await mounted({
      tickets: [...boardTickets(), ticket({ id: 12, number: 16, state: 'draft' })],
    })

    await dragCard(
      wrapper.get('[data-testid="kanban-card-7"]'),
      wrapper.get('[data-testid="kanban-column-draft"]'),
    )
    await flushPromises()

    expect(command).not.toHaveBeenCalled()
    expect(wrapper.get('[data-testid="board-notice"]').text()).toContain('Nothing moves into Draft')
    expect(wrapper.find('[data-testid="kanban-column-backlog"] [data-testid="kanban-card-7"]').exists()).toBe(true)
  })

  it('shows and hides Draft from the toolbar, drifting the view', async () => {
    const { wrapper, command } = await mounted()
    expect(wrapper.find('[data-testid="kanban-column-draft"]').exists()).toBe(false)
    expect(wrapper.get('[data-testid="draft-count"]').text()).toBe('0')
    expect(wrapper.find('[data-testid="view-drift"]').exists()).toBe(false)

    await wrapper.get('[data-testid="toggle-draft"]').trigger('click')

    expect(wrapper.find('[data-testid="kanban-column-draft"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="view-drift"]').exists()).toBe(true)
    // A presentation change is a working change until saved.
    expect(command).not.toHaveBeenCalled()
  })

  it('opens Backlog and Staged into their states and aggregates them back', async () => {
    const { wrapper } = await mounted()

    await wrapper.get('[data-testid="layout-axis-backlog-expanded"]').trigger('click')
    expect(wrapper.find('[data-testid="kanban-column-ready"] [data-testid="kanban-card-7"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="kanban-column-parked"]').exists()).toBe(true)
    expect(wrapper.get('[data-testid="kanban-group-backlog"]').attributes('data-grouped')).toBe('true')

    await wrapper.get('[data-testid="layout-axis-completion-expanded"]').trigger('click')
    expect(wrapper.find('[data-testid="kanban-column-approved"] [data-testid="kanban-card-9"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="kanban-column-landing"]').exists()).toBe(true)

    await wrapper.get('[data-testid="layout-axis-backlog-collapsed"]').trigger('click')
    expect(wrapper.find('[data-testid="kanban-column-parked"]').exists()).toBe(false)
    expect(wrapper.find('[data-testid="kanban-column-backlog"] [data-testid="kanban-card-7"]').exists()).toBe(true)
  })

  it('collapses a column to a named 48px rail with its live count, and expands it back', async () => {
    const { wrapper } = await mounted()

    await wrapper.get('[data-testid="column-collapse-backlog"]').trigger('click')

    const column = wrapper.get('[data-testid="kanban-column-backlog"]')
    expect(column.attributes('data-collapsed')).toBe('true')
    expect(column.attributes('style')).toContain('48px')
    expect(wrapper.find('[data-testid="kanban-card-7"]').exists()).toBe(false)
    const rail = wrapper.get('[data-testid="column-rail-backlog"]')
    expect(rail.text()).toContain('Backlog')
    expect(wrapper.get('[data-testid="column-rail-count-backlog"]').text()).toBe('1')
    // Collapsing changes presentation only; the view did not drift.
    expect(wrapper.find('[data-testid="view-drift"]').exists()).toBe(false)
    // The arrangement is the core's record, not the browser's.
    expect(localStorage.length).toBe(0)

    await wrapper.get('[data-testid="column-expand-backlog"]').trigger('click')
    expect(wrapper.get('[data-testid="kanban-column-backlog"]').attributes('data-collapsed')).toBe('false')
    expect(wrapper.find('[data-testid="kanban-card-7"]').exists()).toBe(true)
  })

  it('keeps a collapsed column collapsed across a reload, through the core', async () => {
    const shell = harness({ tickets: boardTickets() })
    const first = await mountBoard(shell.transport)
    await first.get('[data-testid="column-collapse-review"]').trigger('click')
    await flushPromises()
    expect(shell.command).toHaveBeenCalledWith(
      'shell.preferences.update',
      expect.objectContaining({
        collapsed_columns: [{ scope: { project: 1 }, columns: ['review'] }],
      }),
    )
    first.unmount()
    mountedBoards.splice(mountedBoards.indexOf(first), 1)

    // A fresh mount over the same core is what a reload is; the
    // browser keeps none of it.
    expect(localStorage.length).toBe(0)
    const wrapper = await mountBoard(shell.transport)
    expect(wrapper.get('[data-testid="kanban-column-review"]').attributes('data-collapsed')).toBe('true')
    expect(wrapper.get('[data-testid="columns-open"]').text()).toContain('1 collapsed')
  })

  it('keeps one Project\'s collapse out of another\'s', async () => {
    const shell = harness({ tickets: boardTickets() })
    const first = await mountBoard(shell.transport, '/projects/1/board')
    await first.get('[data-testid="column-collapse-review"]').trigger('click')
    await flushPromises()

    await router.push('/projects/2/board')
    await flushPromises()

    expect(first.get('[data-testid="kanban-column-review"]').attributes('data-collapsed')).toBe('false')
  })

  it('hides a column from the Columns flyout, which removes it, unlike collapse', async () => {
    const { wrapper } = await mounted()

    await wrapper.get('[data-testid="columns-open"]').trigger('click')
    const flyout = wrapper.get('[data-testid="columns-flyout"]')
    expect(flyout.attributes('role')).toBe('dialog')
    expect(flyout.get('[data-testid="column-pref-hide-review"]').text()).toBe('Visible')
    expect(flyout.get('[data-testid="column-pref-collapse-review"]').text()).toBe('Expanded')

    await flyout.get('[data-testid="column-pref-hide-review"]').trigger('click')
    expect(wrapper.find('[data-testid="kanban-column-review"]').exists()).toBe(false)
    expect(wrapper.get('[data-testid="column-pref-hide-review"]').text()).toBe('Hidden')
    expect(wrapper.find('[data-testid="view-drift"]').exists()).toBe(true)

    await wrapper.get('[data-testid="column-pref-collapse-current"]').trigger('click')
    expect(wrapper.get('[data-testid="kanban-column-current"]').attributes('data-collapsed')).toBe('true')
    expect(wrapper.get('[data-testid="column-pref-collapse-current"]').text()).toBe('Collapsed')

    await wrapper.get('[data-testid="columns-show-all"]').trigger('click')
    expect(wrapper.find('[data-testid="kanban-column-review"]').exists()).toBe(true)
    expect(wrapper.get('[data-testid="kanban-column-current"]').attributes('data-collapsed')).toBe('false')

    await wrapper.get('[data-testid="columns-close"]').trigger('click')
    expect(wrapper.find('[data-testid="columns-flyout"]').exists()).toBe(false)
  })

  it('refuses a drop on a collapsed column without sending anything', async () => {
    const { wrapper, command } = await mounted()
    await wrapper.get('[data-testid="column-collapse-current"]').trigger('click')

    await dragCard(
      wrapper.get('[data-testid="kanban-card-7"]'),
      wrapper.get('[data-testid="kanban-column-current"]'),
    )
    await flushPromises()

    expect(command).not.toHaveBeenCalledWith('ticket.transition', expect.anything())
    expect(wrapper.get('[data-testid="board-notice"]').text()).toContain('Expand the Current column')
    expect(wrapper.get('[data-testid="board-notice"]').text()).toContain('CORE-T12')
    expect(wrapper.find('[data-testid="kanban-column-backlog"] [data-testid="kanban-card-7"]').exists()).toBe(true)
  })

  it('drags only Task Tickets, and a drop asks the core for the column state', async () => {
    const { wrapper, command } = await mounted()
    expect(wrapper.get('[data-testid="kanban-card-7"]').attributes('draggable')).toBe('true')
    expect(wrapper.get('[data-testid="kanban-card-8"]').attributes('draggable')).toBe('false')
    expect(wrapper.get('[data-testid="kanban-card-9"]').attributes('draggable')).toBe('false')

    await dragCard(
      wrapper.get('[data-testid="kanban-card-7"]'),
      wrapper.get('[data-testid="kanban-column-current"]'),
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
    expect(wrapper.find('[data-testid="kanban-column-current"] [data-testid="kanban-card-7"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="kanban-column-backlog"] [data-testid="kanban-card-7"]').exists()).toBe(false)
    expect(wrapper.find('[data-testid="board-error"]').exists()).toBe(false)
  })

  it('reports the core\'s refusal and keeps the authoritative state', async () => {
    const { wrapper, command } = await mounted()
    command.mockImplementation((name: string) =>
      name === 'ticket.transition'
        ? Promise.reject({
            code: 'invalid_request',
            message: 'bug transitions are agent-owned; a human may drag only Task Tickets',
          })
        : Promise.resolve({}),
    )

    await dragCard(
      wrapper.get('[data-testid="kanban-card-9"]'),
      wrapper.get('[data-testid="kanban-column-current"]'),
    )
    await flushPromises()

    expect(command).toHaveBeenCalledWith(
      'ticket.transition',
      expect.objectContaining({ ticket_id: 9, to: 'active' }),
    )
    expect(wrapper.get('[data-testid="board-error"]').text()).toContain('agent-owned')
    expect(wrapper.find('[data-testid="kanban-column-staged"] [data-testid="kanban-card-9"]').exists()).toBe(true)
    expect(wrapper.get('[data-testid="kanban-card-9"]').attributes('data-state')).toBe('approved')
  })

  it('moves Done below the board and brings it back, drifting the view', async () => {
    const { wrapper, command } = await mounted()

    await wrapper.get('[data-testid="move-done-below-board"]').trigger('click')
    await flushPromises()
    expect(wrapper.find('[data-testid="kanban-column-done"]').exists()).toBe(false)
    expect(wrapper.find('[data-testid="done-table"]').exists()).toBe(true)
    expect(wrapper.get('[data-testid="done-count"]').text()).toBe('1')
    expect(wrapper.find('[data-testid="view-drift"]').exists()).toBe(true)
    // Keyboard operators land on the relocated table.
    expect(document.activeElement?.getAttribute('data-testid')).toBe('bring-done-back-to-board')

    await wrapper.get('[data-testid="view-save"]').trigger('click')
    await flushPromises()
    expect(command).toHaveBeenCalledWith(
      'view.update',
      expect.objectContaining({ view_id: 2, done_placement: 'table' }),
    )
    expect(wrapper.find('[data-testid="view-drift"]').exists()).toBe(false)

    await wrapper.get('[data-testid="bring-done-back-to-board"]').trigger('click')
    await flushPromises()
    expect(wrapper.find('[data-testid="kanban-column-done"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="done-table"]').exists()).toBe(false)
    expect(wrapper.find('[data-testid="view-drift"]').exists()).toBe(true)
    expect(document.activeElement?.getAttribute('data-testid')).toBe('move-done-below-board')
  })

  it('orders cards by priority first or readiness first', async () => {
    const ordering = [
      ticket({ id: 21, number: 21, priority: 'normal', state: 'ready' }),
      ticket({ id: 22, number: 22, priority: 'urgent', state: 'parked' }),
      ticket({ id: 23, number: 23, priority: 'low', state: 'ready' }),
    ]
    const { wrapper } = await mounted({ tickets: ordering })
    const numbers = () =>
      wrapper
        .findAll('[data-testid="kanban-column-backlog"] [data-testid^="card-number-"]')
        .map((entry) => entry.text())

    expect(wrapper.get('[data-testid="sort-priority"]').attributes('aria-pressed')).toBe('true')
    expect(numbers()).toEqual(['CORE-T22', 'CORE-T21', 'CORE-T23'])

    await wrapper.get('[data-testid="sort-readiness"]').trigger('click')
    expect(numbers()).toEqual(['CORE-T21', 'CORE-T23', 'CORE-T22'])
    expect(wrapper.find('[data-testid="view-drift"]').exists()).toBe(true)

    await wrapper.get('[data-testid="board-presentation-register"]').trigger('click')
    expect(
      wrapper
        .findAll('[data-testid="register-column-backlog"] [data-testid^="register-row-"]')
        .map((row) => row.attributes('data-testid')),
    ).toEqual(['register-row-21', 'register-row-23', 'register-row-22'])
  })

  it('mirrors the board groups as stacked tables, moving Task rows only', async () => {
    const { wrapper, command } = await mounted()

    await wrapper.get('[data-testid="board-presentation-register"]').trigger('click')
    expect(wrapper.find('[data-testid="board-register"]').exists()).toBe(true)
    expect(
      wrapper
        .findAll('[data-testid^="register-column-"]')
        .map((column) => column.attributes('data-testid')),
    ).toEqual([
      'register-column-backlog',
      'register-column-current',
      'register-column-review',
      'register-column-staged',
      'register-column-done',
    ])
    expect(wrapper.get('[data-testid="register-count-backlog"]').text()).toBe('1')
    expect(wrapper.find('[data-testid="move-7"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="move-8"]').exists()).toBe(false)
    expect(wrapper.get('[data-testid="register-agent-owned-8"]').text()).toContain('Agent-owned')
    expect(wrapper.get('[data-testid="register-agent-owned-9"]').text()).toContain('Agent-owned')

    await wrapper.get('[data-testid="move-7"]').setValue('current')
    await flushPromises()
    expect(command).toHaveBeenCalledWith(
      'ticket.transition',
      expect.objectContaining({ ticket_id: 7, to: 'active' }),
    )
    expect((wrapper.get('[data-testid="move-7"]').element as HTMLSelectElement).value).toBe('')
  })

  it('offers the register only the moves the core says are legal', async () => {
    const { wrapper, query } = await mounted({
      tickets: [...boardTickets(), ticket({ id: 12, number: 16, state: 'draft' })],
    })
    await wrapper.get('[data-testid="board-presentation-register"]').trigger('click')

    // The board asks the core what each Task may reach; it keeps no
    // lifecycle table of its own.
    expect(
      query.mock.calls.filter(([name]) => name === 'ticket.transitions').map(([, request]) => request),
    ).toContainEqual({ ticket_id: 7 })
    // A Ready Task starts work or parks; Review, Staged and Done are
    // not its to reach, and parking is the Backlog column it already
    // sits in while Backlog is aggregated.
    expect(
      wrapper.get('[data-testid="move-7"]').findAll('option').map((option) => option.attributes('value')),
    ).toEqual(['', 'current'])
    // A Draft Task moves into Backlog, never into Draft and never
    // past the states between.
    expect(
      wrapper.get('[data-testid="move-12"]').findAll('option').map((option) => option.attributes('value')),
    ).toEqual(['', 'backlog'])
  })

  it('offers a nested target the aggregated column hides', async () => {
    const { wrapper } = await mounted()
    await wrapper.get('[data-testid="layout-axis-backlog-expanded"]').trigger('click')
    await wrapper.get('[data-testid="board-presentation-register"]').trigger('click')

    expect(
      wrapper.get('[data-testid="move-7"]').findAll('option').map((option) => option.attributes('value')),
    ).toEqual(['', 'parked', 'current'])
  })

  it('offers no register move for a Task the core says has landed', async () => {
    const { wrapper } = await mounted()
    await wrapper.get('[data-testid="board-presentation-register"]').trigger('click')

    // Ticket 10 is a done Task: done is final.
    expect(wrapper.find('[data-testid="move-10"]').exists()).toBe(false)
    expect(wrapper.find('[data-testid="register-row-10"]').exists()).toBe(true)
  })

  it('names expanded states under their group in the register', async () => {
    const { wrapper } = await mounted()
    await wrapper.get('[data-testid="layout-axis-backlog-expanded"]').trigger('click')
    await wrapper.get('[data-testid="board-presentation-register"]').trigger('click')

    expect(wrapper.get('[data-testid="register-column-parked"]').text()).toContain('Backlog · Parked')
    expect(wrapper.find('[data-testid="register-column-ready"] [data-testid="register-row-7"]').exists()).toBe(true)
  })

  it('offers the eight filters, re-queries the core, and shows removable chips', async () => {
    const { wrapper, query } = await mounted()

    await wrapper.get('[data-testid="filters-open"]').trigger('click')
    const flyout = wrapper.get('[data-testid="filters-flyout"]')
    expect(flyout.attributes('role')).toBe('dialog')
    expect(
      flyout.findAll('select').map((select) => select.attributes('aria-label')),
    ).toEqual([
      'Filter by Initiative',
      'Filter by Project',
      'Filter by Kind',
      'Filter by State',
      'Filter by Priority',
      'Filter by Lane',
      'Filter by Profile',
      'Filter by Attention',
    ])
    expect(flyout.get('[data-testid="filter-lanes"]').text()).toContain('CORE lane 5')

    await flyout.get('[data-testid="filter-kinds"]').setValue('bug')
    await flushPromises()

    expect(boardCalls(query).at(-1)).toEqual({ filter: { projects: [1], kinds: ['bug'] } })
    expect(wrapper.find('[data-testid="kanban-card-7"]').exists()).toBe(false)
    expect(wrapper.find('[data-testid="kanban-card-9"]').exists()).toBe(true)
    expect(wrapper.get('[data-testid="board-count"]').text()).toContain('1 of 4')
    expect(wrapper.get('[data-testid="filters-count"]').text()).toContain('1 of 4')
    expect(wrapper.get('[data-testid="filters-badge"]').text()).toBe('1')
    expect(wrapper.find('[data-testid="view-drift"]').exists()).toBe(true)

    await wrapper.get('[data-testid="filters-close"]').trigger('click')
    const chip = wrapper.get('[data-testid="filter-chip-kinds-bug"]')
    expect(chip.text()).toContain('Bug')
    await chip.trigger('click')
    await flushPromises()

    expect(boardCalls(query).at(-1)).toEqual({ filter: { projects: [1] } })
    expect(wrapper.find('[data-testid="filter-chip-kinds-bug"]').exists()).toBe(false)
    expect(wrapper.find('[data-testid="kanban-card-7"]').exists()).toBe(true)
  })

  it('says when the filters match nothing and clears them in place', async () => {
    const { wrapper } = await mounted()
    await wrapper.get('[data-testid="filters-open"]').trigger('click')
    await wrapper.get('[data-testid="filter-priorities"]').setValue('urgent')
    await flushPromises()
    await wrapper.get('[data-testid="filters-close"]').trigger('click')

    expect(wrapper.get('[data-testid="board-filtered-empty"]').text()).toContain('No tickets match')
    await wrapper.get('[data-testid="filters-clear-inline"]').trigger('click')
    await flushPromises()

    expect(wrapper.find('[data-testid="board-filtered-empty"]').exists()).toBe(false)
    expect(wrapper.find('[data-testid="kanban-card-7"]').exists()).toBe(true)
  })

  it('restores a Saved View whole, keeps drift until saved, and survives a reload', async () => {
    const shell = harness({
      tickets: boardTickets(),
      views: [
        ...(await import('../test/shell-harness')).defaultViews(),
        {
          id: 8,
          name: 'Review queue',
          scope: { project: 1 },
          filter: { projects: [1], states: ['in_review', 'approved'] },
          expanded_groups: ['staged'],
          hidden_columns: ['draft', 'done'],
          mode: 'register',
          done_placement: 'column',
          sorting: 'readiness',
          is_default: false,
          version: 4,
        },
      ],
    })
    const wrapper = await mountBoard(shell.transport)

    await wrapper.get('[data-testid="board-view-select"]').setValue('8')
    await flushPromises()

    expect(boardCalls(shell.query).at(-1)).toEqual({
      filter: { projects: [1], states: ['in_review', 'approved'] },
    })
    expect(wrapper.find('[data-testid="board-register"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="register-column-approved"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="register-column-done"]').exists()).toBe(false)
    expect(wrapper.get('[data-testid="sort-readiness"]').attributes('aria-pressed')).toBe('true')
    expect(wrapper.find('[data-testid="filter-chip-states-in_review"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="view-drift"]').exists()).toBe(false)

    await wrapper.get('[data-testid="sort-priority"]').trigger('click')
    expect(wrapper.find('[data-testid="view-drift"]').exists()).toBe(true)
    await wrapper.get('[data-testid="view-reset"]').trigger('click')
    expect(wrapper.find('[data-testid="view-drift"]').exists()).toBe(false)
    expect(wrapper.get('[data-testid="sort-readiness"]').attributes('aria-pressed')).toBe('true')

    await wrapper.get('[data-testid="board-presentation-board"]').trigger('click')
    await wrapper.get('[data-testid="view-save"]').trigger('click')
    await flushPromises()
    expect(shell.command).toHaveBeenCalledWith(
      'view.update',
      expect.objectContaining({
        view_id: 8,
        mode: 'board',
        sorting: 'readiness',
        expanded_groups: ['staged'],
        hidden_columns: ['draft', 'done'],
        mutation: expect.objectContaining({ optimistic_version: 4 }),
      }),
    )
    wrapper.unmount()
    mountedBoards.splice(mountedBoards.indexOf(wrapper), 1)

    // A fresh mount over the same core reads the saved perspective back.
    const again = await mountBoard(shell.transport)
    await again.get('[data-testid="board-view-select"]').setValue('8')
    await flushPromises()
    expect(again.find('[data-testid="board-register"]').exists()).toBe(false)
    expect(again.find('[data-testid="kanban-board"]').exists()).toBe(true)
    expect(again.get('[data-testid="sort-readiness"]').attributes('aria-pressed')).toBe('true')
  })

  it('saves the working perspective as a new named view of the scope', async () => {
    const { wrapper, command } = await mounted()
    await wrapper.get('[data-testid="sort-readiness"]').trigger('click')

    await wrapper.get('[data-testid="view-save-as"]').trigger('click')
    await wrapper.get('[data-testid="save-view-name"]').setValue('Readiness sweep')
    await wrapper.get('[data-testid="save-view-create"]').trigger('click')
    await flushPromises()

    expect(command).toHaveBeenCalledWith(
      'view.create',
      expect.objectContaining({ scope: { project: 1 }, name: 'Readiness sweep', sorting: 'readiness' }),
    )
    expect((wrapper.get('[data-testid="board-view-select"]').element as HTMLSelectElement).value).toBe('21')
    expect(wrapper.find('[data-testid="view-drift"]').exists()).toBe(false)
  })

  it('opens a card in the drawer with real detail, and shows its loading and failure states', async () => {
    const { wrapper } = await mounted()

    await wrapper.get('[data-testid="open-ticket-7"]').trigger('click')
    await flushPromises()

    const heading = document.querySelector('[role="dialog"] h2')
    expect(heading?.textContent).toContain('CORE-T12')
    expect(heading?.textContent).toContain('Archive the old exports')
    expect(document.querySelector('[data-testid="drawer-state"]')?.textContent).toBe('Ready')
    expect(document.querySelector('[data-testid="drawer-completion"]')?.textContent).toContain(
      'The old exports are archived.',
    )
    expect(document.querySelector('[data-testid="drawer-attempts"]')).not.toBeNull()
    expect(document.querySelector('[data-testid="drawer-timeline"]')).not.toBeNull()
    ;(document.querySelector('[aria-label="Close panel"]') as HTMLElement).click()
    await flushPromises()
    expect(document.querySelector('[role="dialog"]')).toBeNull()

    const failing = await mounted({
      override: (name) =>
        name === 'ticket.get' ? Promise.reject({ code: 'unavailable', message: 'detail is offline' }) : undefined,
    })
    await failing.wrapper.get('[data-testid="open-ticket-7"]').trigger('click')
    await flushPromises()
    expect(document.querySelector('[data-testid="drawer-error"]')?.textContent).toContain('detail is offline')
  })

  it('opens the drawer for the Ticket a link names', async () => {
    const { wrapper } = await mounted({}, '/projects/1/board?ticket=8')

    expect(document.querySelector('[role="dialog"] h2')?.textContent).toContain('CORE-T13')
    expect(wrapper.find('[data-testid="kanban-card-8"]').exists()).toBe(true)
  })

  it('stacks the board down the page at narrow width', async () => {
    const shell = harness({ tickets: boardTickets() })
    const pinia = createPinia()
    const wrapper = await mountBoard(shell.transport, '/projects/1/board', pinia)
    useShellStore(pinia).setNarrow(true)
    await flushPromises()

    // Stacked, not a row the operator has to scroll sideways.
    const board = wrapper.get('[data-testid="kanban-board"]')
    expect(board.classes()).toContain('flex-col')
    expect(board.classes()).not.toContain('overflow-x-auto')
    expect(wrapper.get('[data-testid="kanban-column-current"]').attributes('style')).toContain(
      'width: 100%',
    )

    // A collapsed column keeps its name and its live count across a
    // strip, since the rail down the side has nowhere to stand.
    await wrapper.get('[data-testid="column-collapse-current"]').trigger('click')
    await flushPromises()
    const rail = wrapper.get('[data-testid="column-rail-current"]')
    expect(rail.text()).toContain('Current')
    expect(rail.get('[data-testid="column-rail-count-current"]').text()).toBe('1')
    expect(wrapper.get('[data-testid="kanban-column-current"]').attributes('style')).toContain(
      `height: ${COLLAPSED_ROW_HEIGHT_PX}px`,
    )
  })

  it('says the drawer detail is on its way before it arrives', async () => {
    let answer: ((ticket: TicketRecord) => void) | undefined
    const { wrapper } = await mounted({
      override: (name) =>
        name === 'ticket.get'
          ? new Promise((resolve) => {
              answer = resolve as (ticket: TicketRecord) => void
            })
          : undefined,
    })

    await wrapper.get('[data-testid="open-ticket-7"]').trigger('click')
    await flushPromises()
    expect(document.querySelector('[data-testid="drawer-loading"]')).not.toBeNull()
    expect(document.querySelector('[data-testid="drawer-state"]')).toBeNull()

    answer?.(boardTickets()[0])
    await flushPromises()
    expect(document.querySelector('[data-testid="drawer-loading"]')).toBeNull()
    expect(document.querySelector('[data-testid="drawer-state"]')?.textContent).toBe('Ready')
  })

  it('shows the loading columns while the projection arrives', async () => {
    const shell = harness({
      override: (name) => (name === 'board.global' ? new Promise(() => undefined) : undefined),
    })
    const wrapper = await mountBoard(shell.transport)

    expect(wrapper.find('[data-testid="board-loading"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="kanban-board"]').exists()).toBe(false)
  })

  it('points an empty Project at planning rather than bare columns', async () => {
    const { wrapper } = await mounted({ tickets: [] })

    expect(wrapper.get('[data-testid="board-empty"]').text()).toContain('No tickets in this Project yet')
    expect(wrapper.find('[data-testid="board-empty"] a[href="/planning"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="kanban-board"]').exists()).toBe(false)
  })

  it('reports a failed load', async () => {
    const shell = harness({
      override: (name) =>
        name === 'board.global' ? Promise.reject({ code: 'unavailable', message: 'the core is offline' }) : undefined,
    })
    const wrapper = await mountBoard(shell.transport)

    expect(wrapper.get('[data-testid="board-error"]').text()).toContain('the core is offline')
  })

  it('says which Project it cannot board', async () => {
    const { wrapper } = await mounted({}, '/projects/99/board')

    expect(wrapper.get('[data-testid="board-project-missing"]').text()).toContain('Project 99')
  })

  it('takes the previous scope\'s cards and drawer away when the scope changes', async () => {
    const shell = harness({ tickets: boardTickets() })
    const wrapper = await mountBoard(shell.transport)
    await wrapper.get('[data-testid="open-ticket-7"]').trigger('click')
    await flushPromises()
    expect(document.querySelector('[role="dialog"]')).not.toBeNull()

    shell.query.mockImplementation(() => new Promise<BoardGlobalResponse>(() => undefined))
    await router.push('/projects/2/board')
    await flushPromises()

    expect(wrapper.find('[data-testid="kanban-card-7"]').exists()).toBe(false)
    expect(wrapper.find('[data-testid="board-loading"]').exists()).toBe(true)
    expect(document.querySelector('[role="dialog"]')).toBeNull()
  })

  it('renders the same projection under either mode from one query', async () => {
    const { wrapper, query } = await mounted()
    const before = boardCalls(query).length

    await wrapper.get('[data-testid="board-presentation-register"]').trigger('click')
    await wrapper.get('[data-testid="board-presentation-board"]').trigger('click')

    expect(boardCalls(query)).toHaveLength(before)
  })

  it('wears the Surface presentation tokens', async () => {
    const { wrapper } = await mounted()
    const board = wrapper.get('[data-testid="kanban-board"]')
    expect(board.classes()).toContain('overflow-x-auto')
    const column = wrapper.get('[data-testid="kanban-column-backlog"]')
    expect(column.classes()).toContain('rounded-panel')
    const card = wrapper.get('[data-testid="kanban-card-7"]')
    expect(card.classes()).toContain('rounded-control')
    expect(card.classes()).toContain('shadow-panel')
    expect(wrapper.get('#kanban-heading-backlog').classes()).toContain('font-display')
  })

  it('wears the Spec and Lane the projection resolved, never a row id', async () => {
    const { wrapper } = await mounted({ extras: { 8: { lane_id: 5, spec_number: 9 } } })
    expect(wrapper.get('[data-testid="card-chip-lane-8"]').text()).toContain('Lane 5')
    expect(wrapper.get('[data-testid="card-chip-spec-8"]').text()).toContain('CORE-S9')
    expect(wrapper.text()).not.toContain('CORE-S4')
    expect(projection([ticket()])[0].group).toBe('backlog')
    expect(boardOptions.lanes[0].label).toBe('CORE lane 5')

    await wrapper.get('[data-testid="open-ticket-8"]').trigger('click')
    await flushPromises()
    expect(document.querySelector('[data-testid="drawer-spec"]')?.textContent).toBe('CORE-S9')
  })

  it('wears the reviewers configured on an Implementation, and none when none are', async () => {
    const { wrapper } = await mounted({
      override: (name, request) => {
        if (name !== 'ticket.review.config') return undefined
        const { ticket_id } = request as { ticket_id: number }
        return Promise.resolve({
          config:
            ticket_id === 8
              ? {
                  ticket_id,
                  version: 1,
                  stages: [
                    {
                      slots: [
                        { occupant: { kind: 'profile', name: 'review-strict' }, requirement: 'required' },
                        { occupant: { kind: 'human' }, requirement: 'required' },
                        { occupant: { kind: 'profile', name: 'deep' }, requirement: 'optional' },
                      ],
                    },
                  ],
                }
              : null,
        })
      },
    })
    expect(wrapper.get('[data-testid="card-chip-reviewers-8"]').text()).toContain('review-strict, Human +1')
    expect(wrapper.find('[data-testid="card-chip-reviewers-9"]').exists()).toBe(false)
  })
  // KAN-T137 fix round 1 regressions.

  it('mounts empty over the board a previous route left behind', async () => {
    const pinia = createPinia()
    const shell = harness({ tickets: boardTickets() })
    const first = await mountBoard(shell.transport, '/projects/1/board', pinia)
    expect(first.find('[data-testid="kanban-card-7"]').exists()).toBe(true)
    first.unmount()
    mountedBoards.splice(mountedBoards.indexOf(first), 1)

    // The next board's prerequisites are still on the wire when it
    // mounts; the Project it left must already be gone.
    let releaseProjects = (): void => undefined
    const held = harness({
      tickets: boardTickets(),
      override: (name) =>
        name === 'project.list'
          ? new Promise((resolve) => {
              releaseProjects = () => resolve({ projects: [coreProject, edgeProject] })
            })
          : undefined,
    })
    const second = await mountBoard(held.transport, '/projects/2/board', pinia)

    expect(second.find('[data-testid="kanban-card-7"]').exists()).toBe(false)
    expect(second.find('[data-testid="board-loading"]').exists()).toBe(true)
    releaseProjects()
    await flushPromises()
    expect(second.find('[data-testid="kanban-card-11"]').exists()).toBe(true)
  })

  it('leaves no card actionable when the route names a Project that is not registered', async () => {
    const pinia = createPinia()
    const shell = harness({ tickets: boardTickets() })
    const first = await mountBoard(shell.transport, '/projects/1/board', pinia)
    expect(first.find('[data-testid="kanban-card-7"]').exists()).toBe(true)
    first.unmount()
    mountedBoards.splice(mountedBoards.indexOf(first), 1)

    const missing = harness({ tickets: boardTickets() })
    const second = await mountBoard(missing.transport, '/projects/99/board', pinia)

    expect(second.get('[data-testid="board-project-missing"]').text()).toContain('Project 99')
    expect(second.find('[data-testid="kanban-card-7"]').exists()).toBe(false)
    expect(second.find('[data-testid="board-register"]').exists()).toBe(false)
  })

  it('follows the Ticket a link names when only the query changes', async () => {
    const { wrapper, query } = await mounted({}, '/projects/1/board?ticket=7')
    expect(document.querySelector('[role="dialog"] h2')?.textContent).toContain('CORE-T12')

    await router.push('/projects/1/board?ticket=8')
    await flushPromises()

    expect(
      query.mock.calls.filter(([name]) => name === 'ticket.get').map(([, request]) => request),
    ).toContainEqual({ ticket_id: 8 })
    expect(document.querySelector('[role="dialog"] h2')?.textContent).toContain('CORE-T13')

    // Clearing the link closes the drawer it opened.
    await router.push('/projects/1/board')
    await flushPromises()
    expect(document.querySelector('[role="dialog"]')).toBeNull()
    expect(wrapper.exists()).toBe(true)
  })

  it('drops the Ticket from the link when the operator closes the drawer, so the same link opens again', async () => {
    await mounted({}, '/projects/1/board?ticket=7')

    ;(document.querySelector('[aria-label="Close panel"]') as HTMLElement).click()
    await flushPromises()

    expect(document.querySelector('[role="dialog"]')).toBeNull()
    expect(router.currentRoute.value.query.ticket).toBeUndefined()

    await router.push('/projects/1/board?ticket=7')
    await flushPromises()
    expect(document.querySelector('[role="dialog"] h2')?.textContent).toContain('CORE-T12')
  })

  it('ignores a drawer answer for a Ticket the operator has left', async () => {
    const pending: Array<() => void> = []
    const { wrapper } = await mounted({
      override: (name, request) => {
        if (name !== 'ticket.get') return undefined
        const { ticket_id } = request as { ticket_id: number }
        if (ticket_id !== 7) return undefined
        return new Promise((resolve) => {
          pending.push(() => resolve(boardTickets()[0]))
        })
      },
    })

    await wrapper.get('[data-testid="open-ticket-7"]').trigger('click')
    await flushPromises()
    expect(document.querySelector('[data-testid="drawer-loading"]')).not.toBeNull()
    ;(document.querySelector('[aria-label="Close panel"]') as HTMLElement).click()
    await flushPromises()

    await wrapper.get('[data-testid="open-ticket-8"]').trigger('click')
    await flushPromises()
    expect(document.querySelector('[role="dialog"] h2')?.textContent).toContain('CORE-T13')

    pending.splice(0).forEach((release) => release())
    await flushPromises()

    expect(document.querySelector('[role="dialog"] h2')?.textContent).toContain('CORE-T13')
    expect(document.querySelector('[data-testid="drawer-loading"]')).toBeNull()
    expect(document.querySelector('[data-testid="drawer-error"]')).toBeNull()
  })

  it('keeps a failed drawer answer off a Ticket the operator has moved on to', async () => {
    const pending: Array<() => void> = []
    const { wrapper } = await mounted({
      override: (name, request) => {
        if (name !== 'ticket.get') return undefined
        const { ticket_id } = request as { ticket_id: number }
        if (ticket_id !== 7) return undefined
        return new Promise((_resolve, reject) => {
          pending.push(() => reject({ code: 'unavailable', message: 'detail is offline' }))
        })
      },
    })

    await wrapper.get('[data-testid="open-ticket-7"]').trigger('click')
    await flushPromises()
    await wrapper.get('[data-testid="open-ticket-8"]').trigger('click')
    await flushPromises()
    pending.splice(0).forEach((release) => release())
    await flushPromises()

    expect(document.querySelector('[data-testid="drawer-error"]')).toBeNull()
    expect(document.querySelector('[role="dialog"] h2')?.textContent).toContain('CORE-T13')
  })

  it('reads the filtered projection again once a move lands', async () => {
    const { wrapper, query } = await mounted()
    await wrapper.get('[data-testid="filters-open"]').trigger('click')
    await wrapper.get('[data-testid="filter-states"]').setValue('ready')
    await flushPromises()
    await wrapper.get('[data-testid="filters-close"]').trigger('click')
    expect(wrapper.find('[data-testid="kanban-card-7"]').exists()).toBe(true)
    const before = boardCalls(query).length

    await dragCard(
      wrapper.get('[data-testid="kanban-card-7"]'),
      wrapper.get('[data-testid="kanban-column-current"]'),
    )
    await flushPromises()

    expect(boardCalls(query).length).toBeGreaterThan(before)
    expect(boardCalls(query).at(-1)).toEqual({ filter: { projects: [1], states: ['ready'] } })
    // The moved Ticket left the Ready-only projection with the move.
    expect(wrapper.find('[data-testid="kanban-card-7"]').exists()).toBe(false)
  })

  it('reads the board again when the core announces a change, and when the shell reconnects', async () => {
    const { wrapper, query, emit, connection, tickets } = await mounted()
    expect(wrapper.find('[data-testid="kanban-card-12"]').exists()).toBe(false)
    const before = boardCalls(query).length

    const captured = ticket({ id: 12, number: 16, state: 'draft', title: 'Capture the export gap' })
    tickets.push(captured)
    emit({ sequence: 4, event_type: 'ticket.created', payload: captured })
    await flushPromises()

    expect(boardCalls(query).length).toBeGreaterThan(before)
    expect(wrapper.find('[data-testid="kanban-card-12"]').exists()).toBe(true)
    // A populated Draft column reveals itself without being asked.
    expect(wrapper.find('[data-testid="kanban-column-draft"]').exists()).toBe(true)

    const afterEvent = boardCalls(query).length
    connection('connected')
    await flushPromises()
    expect(boardCalls(query).length).toBeGreaterThan(afterEvent)
  })

  it('leaves the board alone for an event that cannot change it', async () => {
    const { query, emit } = await mounted()
    const before = boardCalls(query).length

    emit({
      sequence: 5,
      event_type: 'comment.created',
      payload: {
        id: 1,
        project_id: 1,
        target: { kind: 'ticket', id: 'ticket:7' },
        text: 'noted',
        version: 1,
      },
    })
    await flushPromises()

    expect(boardCalls(query)).toHaveLength(before)
  })

  it('collapses every column an expanded group shows, and says how many are collapsed', async () => {
    const { wrapper } = await mounted()
    await wrapper.get('[data-testid="layout-axis-backlog-expanded"]').trigger('click')
    await wrapper.get('[data-testid="columns-open"]').trigger('click')

    await wrapper.get('[data-testid="column-pref-collapse-backlog"]').trigger('click')

    for (const column of ['parked', 'blocked', 'scheduled', 'ready']) {
      expect(
        wrapper.get(`[data-testid="kanban-column-${column}"]`).attributes('data-collapsed'),
        `${column} follows the group`,
      ).toBe('true')
    }
    expect(wrapper.get('[data-testid="column-pref-collapse-backlog"]').text()).toContain('Collapsed')

    await wrapper.get('[data-testid="column-pref-collapse-backlog"]').trigger('click')
    expect(wrapper.get('[data-testid="kanban-column-ready"]').attributes('data-collapsed')).toBe('false')

    // One nested column collapsed is neither collapsed nor silent.
    await wrapper.get('[data-testid="column-collapse-ready"]').trigger('click')
    expect(wrapper.get('[data-testid="column-pref-collapse-backlog"]').text()).toContain('1 of 4')
  })

  it('selects the layout a segment names, however often it is chosen', async () => {
    const { wrapper } = await mounted()

    await wrapper.get('[data-testid="layout-axis-backlog-collapsed"]').trigger('click')
    expect(wrapper.get('[data-testid="kanban-board"]').attributes('data-backlog-layout')).toBe('collapsed')

    await wrapper.get('[data-testid="layout-axis-backlog-expanded"]').trigger('click')
    await wrapper.get('[data-testid="layout-axis-backlog-expanded"]').trigger('click')
    expect(wrapper.get('[data-testid="kanban-board"]').attributes('data-backlog-layout')).toBe('expanded')

    await wrapper.get('[data-testid="layout-axis-completion-collapsed"]').trigger('click')
    await wrapper.get('[data-testid="layout-axis-completion-collapsed"]').trigger('click')
    expect(wrapper.get('[data-testid="kanban-board"]').attributes('data-completion-layout')).toBe('collapsed')
  })
})
