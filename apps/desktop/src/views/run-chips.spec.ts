// The run chips: the card's profile region speaking the run records
// the core owns (KAN-S9-US3, DR-EP-04). Before dispatch a card shows
// the planned profile; during execution it shows the effective
// profile the run froze, wearing the fallback indicator when the run
// fell back from what the assignment named (DR-BP-12, DR-BP-13).
import { mount, flushPromises } from '@vue/test-utils'
import type { VueWrapper } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import type { ProfileSnapshotRecord, RunRecord } from '@kanban/contracts'
import router from '../router'
import { kanbanTransportKey } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import { useBoardStore } from '../stores/board'
import { harness, ticket } from '../test/shell-harness'
import BoardView from './BoardView.vue'
import { emptyFilter } from './global-board-filters'

const snapshot = (name: string, model: string): ProfileSnapshotRecord => ({
  name,
  harness: 'claude-code',
  model,
  effort: 'high',
  usage_pool: 'operator',
})

const run = (overrides: Partial<RunRecord> = {}): RunRecord => ({
  id: 3,
  project_id: 1,
  ticket_id: 8,
  dispatch_request_id: 4,
  status: 'executing',
  requested: snapshot('glm-implementer', 'opus'),
  effective: snapshot('glm-fallback', 'sonnet'),
  fallback: true,
  fallback_path: ['glm-implementer', 'glm-fallback'],
  created_at: 20,
  version: 1,
  ...overrides,
})

// The executing card every test mounts: an active Implementation with
// a planned profile the run may have fallen back from.
const executingTicket = ticket({
  id: 8,
  number: 13,
  kind: 'implementation',
  state: 'active',
  title: null,
  slice: 'Serve the lifecycle command surface',
  spec_id: null,
  subtype: null,
  mode: null,
  completion: [],
  profile: 'glm-implementer',
  version: 5,
})

beforeEach(() => {
  localStorage.clear()
  document.documentElement.classList.remove('dark')
})

describe('board store runs', () => {
  it('loads the Project\'s runs beside its projection through the generated client', async () => {
    setActivePinia(createPinia())
    const { transport, query } = harness({ tickets: [executingTicket], runs: [run()] })
    const board = useBoardStore()

    await board.refresh(transport, 1, emptyFilter())

    expect(query).toHaveBeenCalledWith('run.list', { project_id: 1 })
    expect(board.runs[1]).toHaveLength(1)
    expect(board.loaded).toBe(true)
    expect(board.error).toBeNull()
  })

  it('answers the execution facts of the Ticket executing now', async () => {
    setActivePinia(createPinia())
    const { transport } = harness({
      tickets: [executingTicket],
      runs: [
        run({ ticket_id: 8, fallback: true }),
        run({ id: 4, ticket_id: 9, fallback: false, effective: snapshot('glm-implementer', 'opus') }),
      ],
    })
    const board = useBoardStore()
    await board.refresh(transport, 1, emptyFilter())

    expect(board.executionFor(8)).toEqual({ effective: 'glm-fallback', fallback: true })
    expect(board.executionFor(9)).toEqual({ effective: 'glm-implementer', fallback: false })
    // A Ticket with no run — before dispatch — has no execution facts.
    expect(board.executionFor(7)).toBeNull()
  })
})

describe('BoardView run chips', () => {
  const mountedBoards: VueWrapper[] = []

  afterEach(() => {
    for (const wrapper of mountedBoards.splice(0)) wrapper.unmount()
    document.body.innerHTML = ''
  })

  async function mountBoard(transport: ShellTransport) {
    await router.push('/projects/1/board')
    await router.isReady()
    const wrapper = mount(BoardView, {
      global: {
        plugins: [createPinia(), router],
        provide: { [kanbanTransportKey as symbol]: transport },
      },
    })
    mountedBoards.push(wrapper)
    await flushPromises()
    return wrapper
  }

  it('shows the effective profile with the fallback indicator during execution', async () => {
    const { transport } = harness({ tickets: [executingTicket], runs: [run({ ticket_id: 8 })] })
    const wrapper = await mountBoard(transport)

    const chip = wrapper.find('[data-testid="card-chip-implementer-8"]')
    expect(chip.text()).toContain('glm-fallback')
    expect(wrapper.find('[data-testid="card-fallback-8"]').exists()).toBe(true)
    expect(chip.attributes('title')).toContain('glm-implementer')
  })

  it('shows the planned profile before dispatch, with no fallback indicator', async () => {
    const { transport } = harness({ tickets: [executingTicket], runs: [] })
    const wrapper = await mountBoard(transport)

    const chip = wrapper.find('[data-testid="card-chip-implementer-8"]')
    expect(chip.text()).toContain('glm-implementer')
    expect(wrapper.find('[data-testid="card-fallback-8"]').exists()).toBe(false)
  })
})
