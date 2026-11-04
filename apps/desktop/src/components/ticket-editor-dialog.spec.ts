// KAN-T139-AC1 and KAN-T139-AC3: the one Ticket editor
// is a dialog, opened from New Ticket, from an uncovered Story, and
// from drawer Edit; create and edit both land through issue.save.
import { flushPromises, mount } from '@vue/test-utils'
import type { VueWrapper } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import type { IssueRecord } from '@kanban/contracts'
import router from '../router'
import { kanbanTransportKey } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import { useIssueOverlayStore } from '../stores/issue-overlay'
import { useTicketDialogStore } from '../stores/ticket-dialog'
import {
  editorHarness,
  editorProject,
  editorSpec,
  implementationTicket,
  otherProject,
} from '../test/editor-harness'
import TicketEditorDialog from './TicketEditorDialog.vue'

const mounted: VueWrapper[] = []

function open(transport: ShellTransport) {
  const wrapper = mount(TicketEditorDialog, {
    global: {
      plugins: [createPinia(), router],
      provide: { [kanbanTransportKey as symbol]: transport },
    },
    attachTo: document.body,
  })
  mounted.push(wrapper)
  return wrapper
}

beforeEach(() => {
  setActivePinia(createPinia())
})

afterEach(() => {
  for (const wrapper of mounted.splice(0)) wrapper.unmount()
  document.body.innerHTML = ''
})

describe('the Ticket editor dialog', () => {
  it('stays shut until an entry point opens it', async () => {
    const { transport } = editorHarness()
    const wrapper = open(transport)
    await flushPromises()

    expect(wrapper.find('[data-testid="ticket-dialog"]').exists()).toBe(false)
  })

  it('New Ticket opens the issue create form on the named Project', async () => {
    const { transport } = editorHarness({ projects: [editorProject, otherProject] })
    const wrapper = open(transport)
    useTicketDialogStore().openCreate({ projectId: 4 })
    await flushPromises()

    const dialog = wrapper.find('[data-testid="ticket-dialog"]')
    expect(dialog.exists()).toBe(true)
    expect(dialog.attributes('role')).toBe('dialog')
    expect(dialog.attributes('aria-modal')).toBe('true')
    const project = wrapper.find('[data-testid="ticket-project"]')
      .element as HTMLSelectElement
    expect(project.value).toBe('4')
    expect(wrapper.find('[data-testid="ticket-kind"]').exists()).toBe(false)
    expect(wrapper.find('[data-testid="ticket-subtype"]').exists()).toBe(false)
    expect(wrapper.find('[data-testid="ticket-mode"]').exists()).toBe(false)
    expect(wrapper.find('[data-testid="ticket-completion"]').exists()).toBe(false)
    expect(wrapper.find('[data-testid="ticket-create"]').text()).toContain('Create issue')
  })

  it('an uncovered Story opens create with the story noted in the description', async () => {
    const { transport } = editorHarness()
    const wrapper = open(transport)
    useTicketDialogStore().openForStory({ projectId: 4, specId: 7, story: 'CORE-S1-US1' })
    await flushPromises()

    expect(wrapper.find('[data-testid="ticket-kind"]').exists()).toBe(false)
    expect(
      (wrapper.find('[data-testid="ticket-description"]').element as HTMLTextAreaElement).value,
    ).toContain('CORE-S1-US1')
  })

  it('drawer Edit reads the issue record and shows the issue form', async () => {
    const { transport, operations } = editorHarness({ tickets: [implementationTicket()] })
    const wrapper = open(transport)
    useTicketDialogStore().openEdit({ projectId: 4, ticketId: 1 })
    await flushPromises()

    expect(operations.some((entry) => entry.name === 'ticket.get')).toBe(true)
    expect(operations.some((entry) => entry.name === 'issue.get')).toBe(true)
    expect(wrapper.find('[data-testid="ticket-kind"]').exists()).toBe(false)
    expect(wrapper.find('[data-testid="ticket-edit-key"]').text()).toContain('CORE-17')
    expect((wrapper.find('[data-testid="ticket-title"]').element as HTMLInputElement).value).toBe(
      'Spec authoring creates content versions end to end',
    )
    expect(wrapper.find('[data-testid="ticket-create"]').exists()).toBe(false)
    expect(wrapper.find('[data-testid="ticket-save-issue"]').exists()).toBe(true)
  })

  it('drawer Edit waits for a free issue read while four are on the wire', async () => {
    const answers: Array<() => void> = []
    const { transport, requests } = editorHarness({
      tickets: [implementationTicket()],
      answer: (name, request) => {
        if (name === 'issue.list_milestones') return Promise.resolve({ milestones: [] })
        if (name !== 'issue.get') return undefined
        const { identifier } = request as { identifier: string }
        if (identifier === 'CORE-17') return undefined
        const number = Number(identifier.split('-')[1])
        return new Promise<IssueRecord>((resolve) =>
          answers.push(() =>
            resolve({
              identifier,
              url: `kanban://issue/${identifier}`,
              ticket_id: 200 + number,
              project_id: 4,
              title: `Issue ${number}`,
              description: '',
              state: 'Draft',
              state_type: 'unstarted',
              priority: 3,
              labels: [],
              relations: [],
              state_history: [],
              version: 1,
              created_at: '2026-01-01T00:00:00Z',
              updated_at: '2026-01-01T00:00:00Z',
            }),
          ),
        )
      },
    })
    const wrapper = open(transport)
    const board = useIssueOverlayStore().refreshIssues(
      transport,
      [1, 2, 3, 4].map((number) => ({
        ticketId: 200 + number,
        projectId: 4,
        projectCode: 'CORE',
        number,
      })),
    )
    await flushPromises()
    expect(requests('issue.get')).toHaveLength(4)

    useTicketDialogStore().openEdit({ projectId: 4, ticketId: 1 })
    await flushPromises()
    expect(requests('issue.get')).toHaveLength(4)
    expect((wrapper.find('[data-testid="ticket-title"]').element as HTMLInputElement).value).toBe('')

    answers.shift()!()
    await flushPromises()
    expect(requests('issue.get')).toContainEqual({ identifier: 'CORE-17' })
    expect((wrapper.find('[data-testid="ticket-title"]').element as HTMLInputElement).value).toBe(
      'Spec authoring creates content versions end to end',
    )

    while (answers.length > 0) {
      answers.shift()!()
      await flushPromises()
    }
    await board
  })

  it('the edit form exposes title and description, not legacy kind fields', async () => {
    const { transport } = editorHarness({ tickets: [implementationTicket()] })
    const wrapper = open(transport)
    useTicketDialogStore().openEdit({ projectId: 4, ticketId: 1 })
    await flushPromises()

    expect(wrapper.find('[data-testid="ticket-title"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="ticket-description"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="ticket-slice"]').exists()).toBe(false)
    expect(wrapper.find('[data-testid="ticket-criteria-fact"]').exists()).toBe(false)
    expect(wrapper.find('[data-testid="ticket-completion"]').exists()).toBe(false)
  })

  it('creating an issue sends issue.save to the picked Project', async () => {
    const { transport, requests } = editorHarness()
    const wrapper = open(transport)
    useTicketDialogStore().openCreate({ projectId: 4 })
    await flushPromises()

    await wrapper.find('[data-testid="ticket-title"]').setValue('Register projects end to end')
    await wrapper.find('[data-testid="ticket-description"]').setValue('Covers registration.')
    await wrapper.find('[data-testid="ticket-priority"]').setValue('2')
    await wrapper.find('[data-testid="ticket-create"]').trigger('submit')
    await flushPromises()

    expect(requests('issue.save')[0]).toMatchObject({
      project_id: 4,
      title: 'Register projects end to end',
      description: 'Covers registration.',
      priority: 2,
      state: 'Draft',
    })
    expect(requests('ticket.create')).toHaveLength(0)
    expect(useTicketDialogStore().editorOpen).toBe(false)
  })

  it('a refused creation reports the core message and keeps the dialog open', async () => {
    const { transport } = editorHarness({
      answer: (name) =>
        name === 'issue.save'
          ? Promise.reject({
              code: 'invalid_request',
              message: 'the title cannot be empty',
            })
          : undefined,
    })
    const wrapper = open(transport)
    useTicketDialogStore().openCreate({ projectId: 4 })
    await flushPromises()

    await wrapper.find('[data-testid="ticket-title"]').setValue(' ')
    await wrapper.find('[data-testid="ticket-create"]').trigger('submit')
    await flushPromises()

    expect(wrapper.find('[data-testid="ticket-error"]').text()).toBe('the title cannot be empty')
    expect(wrapper.find('[data-testid="ticket-dialog"]').exists()).toBe(true)
  })

  it('editing an issue sends issue.save at the version the record was read at', async () => {
    const { transport, requests } = editorHarness({ tickets: [implementationTicket()] })
    const wrapper = open(transport)
    useTicketDialogStore().openEdit({ projectId: 4, ticketId: 1 })
    await flushPromises()

    await wrapper.find('[data-testid="ticket-title"]').setValue('A sharper title')
    await wrapper.find('[data-testid="ticket-description"]').setValue('A sharper slice')
    await wrapper.find('[data-testid="ticket-save-issue"]').trigger('submit')
    await flushPromises()

    expect(requests('issue.save')[0]).toMatchObject({
      mutation: { optimistic_version: 1, idempotency_key: expect.any(String) },
      identifier: 'CORE-17',
      title: 'A sharper title',
      description: 'A sharper slice',
    })
  })

  it('a stale edit reports the core refusal rather than pretending it saved', async () => {
    const { transport } = editorHarness({
      tickets: [implementationTicket()],
      answer: (name) =>
        name === 'issue.save'
          ? Promise.reject({ code: 'stale_version', message: 'the Ticket moved on' })
          : undefined,
    })
    const wrapper = open(transport)
    useTicketDialogStore().openEdit({ projectId: 4, ticketId: 1 })
    await flushPromises()

    await wrapper.find('[data-testid="ticket-title"]').setValue('A sharper title')
    await wrapper.find('[data-testid="ticket-save-issue"]').trigger('submit')
    await flushPromises()

    expect(wrapper.find('[data-testid="ticket-error"]').text()).toBe('the Ticket moved on')
    expect(useTicketDialogStore().editorOpen).toBe(true)
  })

  it('editing only the title of an issue carrying a retired label sends no label change', async () => {
    const { transport, requests } = editorHarness({
      tickets: [implementationTicket()],
      answer: (name) =>
        name === 'issue.get'
          ? Promise.resolve({
              identifier: 'CORE-17',
              url: 'kanban://issue/CORE-17',
              ticket_id: 1,
              project_id: 4,
              title: 'Spec authoring creates content versions end to end',
              description: 'Acceptance criteria and story links live in the description.',
              state: 'Draft',
              state_type: 'unstarted',
              priority: 2,
              labels: ['Retired'],
              relations: [],
              state_history: [],
              version: 1,
              created_at: '2026-01-01T00:00:00Z',
              updated_at: '2026-01-01T00:00:00Z',
            } satisfies IssueRecord)
          : undefined,
    })
    const wrapper = open(transport)
    useTicketDialogStore().openEdit({ projectId: 4, ticketId: 1 })
    await flushPromises()

    await wrapper.find('[data-testid="ticket-title"]').setValue('A sharper title')
    await wrapper.find('[data-testid="ticket-save-issue"]').trigger('submit')
    await flushPromises()

    const sent = requests('issue.save')[0]
    expect(sent).toMatchObject({ identifier: 'CORE-17', title: 'A sharper title' })
    expect(sent).not.toHaveProperty('add_labels')
    expect(sent).not.toHaveProperty('remove_labels')
    expect(useTicketDialogStore().editorOpen).toBe(false)
  })

  it('a saved edit survives a reload because the next open reads the record', async () => {
    const state = editorHarness({ tickets: [implementationTicket()] })
    const first = open(state.transport)
    useTicketDialogStore().openEdit({ projectId: 4, ticketId: 1 })
    await flushPromises()
    await first.find('[data-testid="ticket-title"]').setValue('A sharper title')
    await first.find('[data-testid="ticket-description"]').setValue('A sharper slice')
    await first.find('[data-testid="ticket-save-issue"]').trigger('submit')
    await flushPromises()
    first.unmount()

    setActivePinia(createPinia())
    const second = open(state.transport)
    useTicketDialogStore().openEdit({ projectId: 4, ticketId: 1 })
    await flushPromises()

    expect((second.find('[data-testid="ticket-title"]').element as HTMLInputElement).value).toBe(
      'A sharper title',
    )
    expect(
      (second.find('[data-testid="ticket-description"]').element as HTMLTextAreaElement).value,
    ).toBe('A sharper slice')
  })

  it('shows a project-missing alert and does not send issue.save when no project exists', async () => {
    const { transport, operations } = editorHarness({ projects: [] })
    const wrapper = open(transport)
    useTicketDialogStore().openCreate({ projectId: null })
    await flushPromises()

    expect(wrapper.find('[data-testid="ticket-project-missing"]').exists()).toBe(true)
    await wrapper.find('form').trigger('submit')
    await flushPromises()

    expect(wrapper.find('[data-testid="ticket-error"]').text()).toContain(
      'Register a project before creating an issue.',
    )
    expect(operations.filter((entry) => entry.name === 'issue.save')).toHaveLength(0)
  })

  it('create mode never surfaces Bug qualification controls', async () => {
    const { transport, requests } = editorHarness({
      projects: [editorProject, otherProject],
      specs: [editorSpec],
    })
    const wrapper = open(transport)
    useTicketDialogStore().openCreate({ projectId: 4 })
    await flushPromises()

    expect(wrapper.find('[data-testid="bug-qualify"]').exists()).toBe(false)
    await wrapper.find('[data-testid="ticket-project"]').setValue('5')
    await flushPromises()
    expect(wrapper.find('[data-testid="bug-qualify"]').exists()).toBe(false)
    expect(requests('ticket.bug.qualify')).toHaveLength(0)
  })

  it('closing the dialog leaves the record untouched', async () => {
    const { transport, operations } = editorHarness({ tickets: [implementationTicket()] })
    const wrapper = open(transport)
    useTicketDialogStore().openEdit({ projectId: 4, ticketId: 1 })
    await flushPromises()

    await wrapper.find('[data-testid="ticket-title"]').setValue('Abandoned')
    await wrapper.find('[data-testid="ticket-dialog-close"]').trigger('click')
    await flushPromises()

    expect(wrapper.find('[data-testid="ticket-dialog"]').exists()).toBe(false)
    expect(operations.filter((entry) => entry.kind === 'command')).toEqual([])
  })
})
