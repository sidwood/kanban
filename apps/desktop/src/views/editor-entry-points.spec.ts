// KAN-T139-AC1 (KAN-S4-US1): the Ticket editor reaches the operator
// through context and a shortcut alone — New Ticket on the board, an
// uncovered Story on planning, Edit in the drawer. No route, rail
// item, or palette row opens an editor destination.
import { flushPromises, mount } from '@vue/test-utils'
import type { VueWrapper } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import type { Pinia } from 'pinia'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { defineComponent, h } from 'vue'
import type {
  IssueLabelRecord,
  IssueMilestoneRecord,
  IssueSaveRequest,
  ProjectListResponse,
  SpecCoverageMatrixResponse,
  TicketCreateRequest,
  TicketGraphListResponse,
  TicketRecord,
} from '@kanban/contracts'
import router from '../router'
import { kanbanTransportKey } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import { PALETTE_NAVIGATION } from '../stores/palette-navigation'
import { useTicketDialogStore } from '../stores/ticket-dialog'
import { harness, ticket } from '../test/shell-harness'
import App from '../App.vue'
import TicketEditorDialog from '../components/TicketEditorDialog.vue'
import PlanningView from './PlanningView.vue'

const mounted: VueWrapper[] = []

beforeEach(() => {
  localStorage.clear()
  document.documentElement.classList.remove('dark')
})

afterEach(() => {
  for (const wrapper of mounted.splice(0)) wrapper.unmount()
  document.body.innerHTML = ''
})

async function mountApp(transport: ShellTransport, path: string, pinia: Pinia) {
  await router.push(path)
  await router.isReady()
  const wrapper = mount(App, {
    attachTo: document.body,
    global: {
      plugins: [pinia, router],
      provide: { [kanbanTransportKey as symbol]: transport },
    },
  })
  mounted.push(wrapper)
  await flushPromises()
  return wrapper
}

describe('the Ticket editor entry points', () => {
  it('exposes no editor route, rail item, or palette row', () => {
    const paths = router.getRoutes().map((entry) => entry.path)
    expect(paths).not.toContain('/planning/tickets')
    expect(router.resolve('/planning/tickets').matched).toHaveLength(0)
    expect(PALETTE_NAVIGATION.map((item) => item.route)).not.toContain('/planning/tickets')
  })

  it('New ticket on the board opens the dialog on the scoped Project', async () => {
    const pinia = createPinia()
    const wrapper = await mountApp(
      harness({ tickets: [ticket()] }).transport,
      '/projects/1/board',
      pinia,
    )

    const button = wrapper.get('[data-testid="new-ticket"]')
    expect(button.attributes('href')).toBeUndefined()
    await button.trigger('click')
    await flushPromises()

    const dialog = useTicketDialogStore(pinia)
    expect(dialog.editorOpen).toBe(true)
    expect(dialog.editor).toMatchObject({ mode: 'create', projectId: 1 })
    expect(wrapper.find('[data-testid="ticket-dialog"]').exists()).toBe(true)
  })

  // KAN-T139-AC3: what the dialog saves is the core's, so a fresh
  // shell reads it back without the dialog's draft surviving anything.
  it('a Ticket created in the dialog survives a reload of the shell', async () => {
    const state = harness({ tickets: [ticket()] })
    const pinia = createPinia()
    const wrapper = await mountApp(state.transport, '/projects/1/board', pinia)

    await wrapper.get('[data-testid="new-ticket"]').trigger('click')
    await flushPromises()
    await wrapper.find('[data-testid="ticket-title"]').setValue('Archive the old exports')
    await wrapper.find('[data-testid="ticket-description"]').setValue('The old exports are archived.')
    await wrapper.find('[data-testid="ticket-create"]').trigger('submit')
    await flushPromises()

    expect(useTicketDialogStore(pinia).editorOpen).toBe(false)
    const created = state.tickets[state.tickets.length - 1]!
    expect(created.title).toBe('Archive the old exports')

    // A fresh shell over the same core: the card is there because the
    // record is, not because anything was kept on the client.
    wrapper.unmount()
    mounted.splice(mounted.indexOf(wrapper), 1)
    const reloaded = await mountApp(state.transport, '/projects/1/board', createPinia())
    expect(reloaded.find(`[data-testid="kanban-card-${created.id}"]`).exists()).toBe(true)
  })

  it('the global shortcut opens quick Bug capture, and nothing else does', async () => {
    const pinia = createPinia()
    const wrapper = await mountApp(harness({ tickets: [ticket()] }).transport, '/board', pinia)
    const dialog = useTicketDialogStore(pinia)
    expect(dialog.quickBugOpen).toBe(false)

    window.dispatchEvent(
      new KeyboardEvent('keydown', { key: 'b', metaKey: true, shiftKey: true }),
    )
    await flushPromises()

    expect(dialog.quickBugOpen).toBe(true)
    expect(wrapper.find('[data-testid="quick-bug-dialog"]').exists()).toBe(true)
  })
})

// The planning surface's own entry point: a Story no Ticket covers
// offers the Implementation that would cover it.
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

const matrix: SpecCoverageMatrixResponse = {
  spec_id: 1,
  version: 2,
  stories: [
    {
      story: 'CORE-S1-US1',
      claims: [{ ticket_id: 5, ticket_number: 5, outcome: 'Graphs record completely.' }],
    },
    { story: 'CORE-S1-US3', claims: [] },
  ],
}

function planningHarness(): ShellTransport {
  return {
    query: (name: string) => {
      if (name === 'project.list') return Promise.resolve({ projects: [project] })
      if (name === 'spec.list') {
        return Promise.resolve({
          specs: [
            {
              id: 1,
              project_id: 4,
              number: 1,
              name: 'Registration',
              execution: 'planned',
              plan_id: 1,
              version: 3,
            },
          ],
        })
      }
      if (name === 'spec.coverage.matrix') return Promise.resolve(matrix)
      if (name === 'plan.list') return Promise.resolve({ plans: [] })
      if (name === 'ticket.list') return Promise.resolve({ tickets: [] })
      if (name === 'ticket.graph.list') return Promise.resolve({ proposals: [] })
      return Promise.resolve({})
    },
    command: () => Promise.resolve({}),
    subscribe: () => () => undefined,
    onConnectionChange: () => () => undefined,
  } as unknown as ShellTransport
}

describe('an uncovered Story', () => {
  it('opens the dialog preset to an Implementation on that Spec and Story', async () => {
    const pinia = createPinia()
    setActivePinia(pinia)
    await router.push('/planning')
    await router.isReady()
    const wrapper = mount(PlanningView, {
      global: {
        plugins: [pinia, router],
        provide: { [kanbanTransportKey as symbol]: planningHarness() },
      },
    })
    mounted.push(wrapper)
    await flushPromises()

    // A covered Story offers nothing; only the gap does.
    expect(wrapper.find('[data-testid="coverage-cover-CORE-S1-US1"]').exists()).toBe(false)
    await wrapper.find('[data-testid="coverage-cover-CORE-S1-US3"]').trigger('click')
    await flushPromises()

    expect(useTicketDialogStore(pinia).editor).toMatchObject({
      mode: 'create',
      projectId: 4,
      specId: 1,
      kind: 'implementation',
      kindLocked: true,
      story: 'CORE-S1-US3',
    })
  })
})

// A core that keeps what `ticket.create` mints and answers the
// coverage matrix from the criteria its Implementations hold, so a
// read after the create reports what the create actually claimed. Like
// the real core it refuses a criterion that states no outcome, and it
// keeps the issue fields a later `issue.save` gives a created Ticket.
function coveringCore(options: { refusedFieldSaves?: number } = {}) {
  const tickets: TicketRecord[] = []
  const commands: Array<{ name: string; request: unknown }> = []
  const issueFields: Record<string, IssueSaveRequest> = {}
  const stories = ['CORE-S1-US1', 'CORE-S1-US3']
  let refusedFieldSaves = options.refusedFieldSaves ?? 0
  const labels: IssueLabelRecord[] = [{ name: 'Urgent work', color: '#d33' }]
  const milestones: IssueMilestoneRecord[] = [
    { id: 7, name: 'Launch', issues_done: 0, issues_total: 0 },
  ]
  const transport = {
    query: (name: string) => {
      if (name === 'spec.coverage.matrix') {
        return Promise.resolve({
          spec_id: 1,
          version: 2,
          stories: stories.map((story) => ({
            story,
            claims: tickets.flatMap((held) =>
              held.kind === 'implementation' && held.spec_id === 1
                ? held.criteria
                    .filter((criterion) => criterion.stories.includes(story))
                    .map((criterion) => ({
                      ticket_id: held.id,
                      ticket_number: held.number,
                      outcome: criterion.outcome,
                    }))
                : [],
            ),
          })),
        } satisfies SpecCoverageMatrixResponse)
      }
      if (name === 'ticket.list') return Promise.resolve({ tickets: [...tickets] })
      if (name === 'issue.list_labels') return Promise.resolve({ labels })
      if (name === 'issue.list_milestones') return Promise.resolve({ milestones })
      if (name === 'ticket.graph.list') {
        return Promise.resolve({
          proposals: [
            { id: 9, spec_id: 1, spec_version: 2, state: 'proposed', tickets: [], edges: [], version: 1 },
          ],
        } satisfies TicketGraphListResponse)
      }
      return planningHarness().query(name as never, {})
    },
    command: (name: string, request: unknown) => {
      commands.push({ name, request })
      if (name === 'issue.save') {
        const save = request as IssueSaveRequest
        const standing = tickets.find((held) => `CORE-${held.number}` === save.identifier)
        if (!standing) return Promise.reject({ code: 'not_found', message: 'issue' })
        if (refusedFieldSaves > 0) {
          refusedFieldSaves -= 1
          return Promise.reject({ code: 'unavailable', message: 'the core is busy' })
        }
        if (save.mutation.optimistic_version !== standing.version) {
          return Promise.reject({ code: 'stale_version', message: 'the Ticket moved on' })
        }
        issueFields[save.identifier!] = save
        tickets.splice(tickets.indexOf(standing), 1, {
          ...standing,
          slice: save.title ?? standing.slice,
          version: standing.version + 1,
        })
        return Promise.resolve({
          identifier: save.identifier,
          ticket_id: standing.id,
          url: `kanban://issue/${save.identifier}`,
        })
      }
      if (name !== 'ticket.create') return Promise.resolve({})
      const create = request as TicketCreateRequest
      if ((create.criteria ?? []).some((criterion) => criterion.outcome.trim() === '')) {
        return Promise.reject({
          code: 'invalid_request',
          message: 'an Acceptance Criterion states an observable outcome',
        })
      }
      const created = ticket({
        id: 40 + tickets.length,
        project_id: create.project_id,
        number: 3 + tickets.length,
        kind: create.kind,
        priority: create.priority,
        state: 'draft',
        spec_id: create.spec_id ?? null,
        title: null,
        slice: create.slice ?? null,
        criteria: create.criteria ?? [],
        subtype: null,
        mode: null,
        completion: [],
        version: 1,
      })
      tickets.push(created)
      return Promise.resolve(created)
    },
    subscribe: () => () => undefined,
    onConnectionChange: () => () => undefined,
  } as unknown as ShellTransport
  return { transport, commands, tickets, issueFields }
}

async function mountCovering(core: ReturnType<typeof coveringCore>) {
  const pinia = createPinia()
  setActivePinia(pinia)
  await router.push('/planning')
  await router.isReady()
  const surface = defineComponent({
    render: () => [h(PlanningView), h(TicketEditorDialog)],
  })
  const wrapper = mount(surface, {
    attachTo: document.body,
    global: {
      plugins: [pinia, router],
      provide: { [kanbanTransportKey as symbol]: core.transport },
    },
  })
  mounted.push(wrapper)
  await flushPromises()
  expect(wrapper.find('[data-testid="coverage-gap-CORE-S1-US3"]').exists()).toBe(true)
  expect(wrapper.find('[data-testid="graph-blocking-9"]').text()).toContain('CORE-S1-US3')
  await wrapper.find('[data-testid="coverage-cover-CORE-S1-US3"]').trigger('click')
  await flushPromises()
  return { wrapper, pinia }
}

function expectStoryCovered(wrapper: VueWrapper, number: number, outcome: string): void {
  expect(wrapper.find('[data-testid="coverage-gap-CORE-S1-US3"]').exists()).toBe(false)
  expect(wrapper.find('[data-testid="coverage-cover-CORE-S1-US3"]').exists()).toBe(false)
  expect(wrapper.find(`[data-testid="coverage-claim-CORE-S1-US3-${number}"]`).text()).toContain(
    outcome,
  )
  expect(wrapper.find('[data-testid="graph-blocking-9"]').text()).not.toContain('CORE-S1-US3')
}

describe('covering a Story from planning', () => {
  it('creates the Implementation on the Spec with the Story claimed, and the matrix reads it covered', async () => {
    const core = coveringCore()
    const { wrapper, pinia } = await mountCovering(core)

    await wrapper.find('[data-testid="ticket-title"]').setValue('Registration reports its gaps')
    await wrapper.find('[data-testid="ticket-priority"]').setValue('2')
    await wrapper.find('[data-testid="ticket-create"]').trigger('submit')
    await flushPromises()

    expect(core.commands.map((entry) => entry.name)).toEqual(['ticket.create', 'issue.save'])
    expect(core.commands[0]!.request).toMatchObject({
      project_id: 4,
      kind: 'implementation',
      priority: 'high',
      spec_id: 1,
      slice: 'Registration reports its gaps',
      criteria: [{ outcome: 'Covers story CORE-S1-US3.', stories: ['CORE-S1-US3'] }],
    })
    expect(core.commands[1]!.request).toMatchObject({
      mutation: { optimistic_version: 1 },
      identifier: 'CORE-3',
      description: 'Covers story CORE-S1-US3.',
      priority: 2,
    })
    expect(useTicketDialogStore(pinia).editorOpen).toBe(false)
    expectStoryCovered(wrapper, core.tickets[0]!.number, 'Covers story CORE-S1-US3.')
  })

  it('keeps the description, priority, due date, labels and milestone chosen for the cover', async () => {
    const core = coveringCore()
    const { wrapper, pinia } = await mountCovering(core)

    await wrapper.find('[data-testid="ticket-title"]').setValue('  Registration reports its gaps  ')
    await wrapper.find('[data-testid="ticket-description"]').setValue('  Gaps\nare reported  ')
    await wrapper.find('[data-testid="ticket-priority"]').setValue('0')
    await wrapper.find('[data-testid="ticket-due"]').setValue('2026-12-01')
    await wrapper.find('[data-testid="ticket-milestone"]').setValue('7')
    await wrapper.find('[data-testid="ticket-label-Urgent work"]').setValue(true)
    await wrapper.find('[data-testid="ticket-create"]').trigger('submit')
    await flushPromises()

    expect(core.commands.map((entry) => entry.name)).toEqual(['ticket.create', 'issue.save'])
    expect(core.commands[0]!.request).toMatchObject({
      kind: 'implementation',
      spec_id: 1,
      slice: '  Registration reports its gaps  ',
      criteria: [{ outcome: '  Gaps\nare reported  ', stories: ['CORE-S1-US3'] }],
    })
    expect(core.issueFields['CORE-3']).toEqual({
      mutation: { optimistic_version: 1, idempotency_key: expect.any(String) },
      identifier: 'CORE-3',
      title: '  Registration reports its gaps  ',
      description: '  Gaps\nare reported  ',
      priority: 0,
      due_date: '2026-12-01',
      add_labels: ['Urgent work'],
      milestone_id: 7,
    })
    expect(useTicketDialogStore(pinia).editorOpen).toBe(false)
    expectStoryCovered(wrapper, core.tickets[0]!.number, 'Gaps')
  })

  it('covers the Story when the description is left empty', async () => {
    const core = coveringCore()
    const { wrapper, pinia } = await mountCovering(core)

    await wrapper.find('[data-testid="ticket-title"]').setValue('Registration reports its gaps')
    await wrapper.find('[data-testid="ticket-description"]').setValue('')
    await wrapper.find('[data-testid="ticket-create"]').trigger('submit')
    await flushPromises()

    expect(wrapper.find('[data-testid="ticket-error"]').exists()).toBe(false)
    expect(core.commands.map((entry) => entry.name)).toEqual(['ticket.create', 'issue.save'])
    expect(core.commands[0]!.request).toMatchObject({
      criteria: [{ outcome: 'Covers story CORE-S1-US3.', stories: ['CORE-S1-US3'] }],
    })
    expect(core.issueFields['CORE-3']).toMatchObject({ description: '', priority: 3 })
    expect(useTicketDialogStore(pinia).editorOpen).toBe(false)
    expectStoryCovered(wrapper, core.tickets[0]!.number, 'Covers story CORE-S1-US3.')
  })

  it('saves only the issue fields again when they failed after the Implementation was created', async () => {
    const core = coveringCore({ refusedFieldSaves: 1 })
    const { wrapper, pinia } = await mountCovering(core)

    await wrapper.find('[data-testid="ticket-title"]').setValue('Registration reports its gaps')
    await wrapper.find('[data-testid="ticket-due"]').setValue('2026-12-01')
    await wrapper.find('[data-testid="ticket-create"]').trigger('submit')
    await flushPromises()

    expect(core.commands.map((entry) => entry.name)).toEqual(['ticket.create', 'issue.save'])
    expect(useTicketDialogStore(pinia).editorOpen).toBe(true)
    const error = wrapper.find('[data-testid="ticket-error"]').text()
    expect(error).toContain('CORE-3')
    expect(error).toContain('the core is busy')

    await wrapper.find('[data-testid="ticket-create"]').trigger('submit')
    await flushPromises()

    expect(core.commands.map((entry) => entry.name)).toEqual([
      'ticket.create',
      'issue.save',
      'issue.save',
    ])
    expect(core.tickets).toHaveLength(1)
    expect(core.issueFields['CORE-3']).toMatchObject({ due_date: '2026-12-01' })
    expect(useTicketDialogStore(pinia).editorOpen).toBe(false)
    expectStoryCovered(wrapper, core.tickets[0]!.number, 'Covers story CORE-S1-US3.')
  })

  it('saves a title corrected after the issue fields were refused', async () => {
    const core = coveringCore({ refusedFieldSaves: 1 })
    const { wrapper, pinia } = await mountCovering(core)

    await wrapper.find('[data-testid="ticket-title"]').setValue('Initial title')
    await wrapper.find('[data-testid="ticket-create"]').trigger('submit')
    await flushPromises()
    expect(useTicketDialogStore(pinia).editorOpen).toBe(true)

    await wrapper.find('[data-testid="ticket-title"]').setValue('Corrected title')
    await wrapper.find('[data-testid="ticket-create"]').trigger('submit')
    await flushPromises()

    expect(core.commands.map((entry) => entry.name)).toEqual([
      'ticket.create',
      'issue.save',
      'issue.save',
    ])
    expect(core.commands[2]!.request).toMatchObject({
      mutation: { optimistic_version: 1 },
      identifier: 'CORE-3',
      title: 'Corrected title',
    })
    expect(core.tickets).toHaveLength(1)
    expect(core.tickets[0]!.slice).toBe('Corrected title')
    expect(useTicketDialogStore(pinia).editorOpen).toBe(false)
    expectStoryCovered(wrapper, core.tickets[0]!.number, 'Covers story CORE-S1-US3.')
  })
})
