// KAN-T139-AC1 and KAN-T139-AC3: the one Ticket editor
// is a dialog, opened from New Ticket, from an uncovered Story, and
// from drawer Edit; create lands through issue.save, edit through
// ticket.edit and Bug qualification commands.
import { flushPromises, mount } from '@vue/test-utils'
import type { VueWrapper } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import router from '../router'
import { kanbanTransportKey } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import { useTicketDialogStore } from '../stores/ticket-dialog'
import {
  capturedBug,
  editorHarness,
  editorProject,
  editorSpec,
  implementationTicket,
  otherProject,
  qualifiedBug,
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

  it('drawer Edit opens it preset to the Ticket kind, read from the record', async () => {
    const { transport, operations } = editorHarness({ tickets: [implementationTicket()] })
    const wrapper = open(transport)
    useTicketDialogStore().openEdit({ projectId: 4, ticketId: 1 })
    await flushPromises()

    expect(operations.some((entry) => entry.name === 'ticket.get')).toBe(true)
    const kind = wrapper.find('[data-testid="ticket-kind"]').element as HTMLSelectElement
    expect(kind.value).toBe('implementation')
    expect(kind.disabled).toBe(true)
    expect((wrapper.find('[data-testid="ticket-slice"]').element as HTMLTextAreaElement).value).toBe(
      'Spec authoring creates content versions end to end',
    )
    expect(wrapper.find('[data-testid="ticket-create"]').exists()).toBe(false)
    expect(wrapper.find('[data-testid="ticket-save"]').exists()).toBe(true)
  })

  it('the edit form follows the standing kind', async () => {
    const { transport } = editorHarness({ tickets: [implementationTicket()] })
    const wrapper = open(transport)
    useTicketDialogStore().openEdit({ projectId: 4, ticketId: 1 })
    await flushPromises()

    expect(wrapper.find('[data-testid="ticket-slice"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="ticket-criteria-fact"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="ticket-title"]').exists()).toBe(false)
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

  it('editing an Implementation sends ticket.edit at the version the record was read at', async () => {
    const { transport, requests } = editorHarness({ tickets: [implementationTicket()] })
    const wrapper = open(transport)
    useTicketDialogStore().openEdit({ projectId: 4, ticketId: 1 })
    await flushPromises()

    await wrapper.find('[data-testid="ticket-slice"]').setValue('A sharper slice')
    await wrapper.find('[data-testid="ticket-save"]').trigger('submit')
    await flushPromises()

    expect(requests('ticket.edit')[0]).toEqual({
      mutation: { optimistic_version: 1, idempotency_key: expect.any(String) },
      ticket_id: 1,
      slice: 'A sharper slice',
    })
  })

  it('a stale edit reports the core refusal rather than pretending it saved', async () => {
    const { transport } = editorHarness({
      tickets: [implementationTicket()],
      answer: (name) =>
        name === 'ticket.edit'
          ? Promise.reject({ code: 'stale_version', message: 'the Ticket moved on' })
          : undefined,
    })
    const wrapper = open(transport)
    useTicketDialogStore().openEdit({ projectId: 4, ticketId: 1 })
    await flushPromises()

    await wrapper.find('[data-testid="ticket-slice"]').setValue('A sharper slice')
    await wrapper.find('[data-testid="ticket-save"]').trigger('submit')
    await flushPromises()

    expect(wrapper.find('[data-testid="ticket-error"]').text()).toBe('the Ticket moved on')
    expect(useTicketDialogStore().editorOpen).toBe(true)
  })

  it('a saved edit survives a reload because the next open reads the record', async () => {
    const state = editorHarness({ tickets: [implementationTicket()] })
    const first = open(state.transport)
    useTicketDialogStore().openEdit({ projectId: 4, ticketId: 1 })
    await flushPromises()
    await first.find('[data-testid="ticket-slice"]').setValue('A sharper slice')
    await first.find('[data-testid="ticket-save"]').trigger('submit')
    await flushPromises()
    first.unmount()

    setActivePinia(createPinia())
    const second = open(state.transport)
    useTicketDialogStore().openEdit({ projectId: 4, ticketId: 1 })
    await flushPromises()

    expect((second.find('[data-testid="ticket-slice"]').element as HTMLTextAreaElement).value).toBe(
      'A sharper slice',
    )
  })

  it('a Bug edit seeds the qualification that stands and sends it whole', async () => {
    const { transport, requests } = editorHarness({ tickets: [qualifiedBug()] })
    const wrapper = open(transport)
    useTicketDialogStore().openEdit({ projectId: 4, ticketId: 19 })
    await flushPromises()

    expect(
      (wrapper.find('[data-testid="bug-qualify-severity"]').element as HTMLSelectElement).value,
    ).toBe('high')
    await wrapper.find('[data-testid="bug-qualify-severity"]').setValue('critical')
    await wrapper.find('[data-testid="bug-qualify"]').trigger('submit')
    await flushPromises()

    expect(requests('ticket.bug.qualify')[0]).toEqual({
      mutation: { optimistic_version: 3, idempotency_key: expect.any(String) },
      ticket_id: 19,
      qualification: {
        expected_behaviour: 'The integration branch survives every landing.',
        reproduction: 'Re land a reviewed change; the branch list still names it.',
        environment: 'macOS 26, Kanban 0.1.0.',
        severity: 'critical',
        frequency: 'Every landing so far.',
        affected_scope: 'All landing reviews.',
        risk: 'Duplicate landings and lost review state.',
        criteria: [
          { outcome: 'The integration branch survives a landing.', stories: ['CORE-S1-US1'] },
        ],
        verification_steps: [{ command: 'cargo test -p kanban-storage tickets' }],
      },
    })
  })

  it('a half-qualified Bug cannot be sent: severity is chosen, never defaulted', async () => {
    const { transport, requests } = editorHarness({ tickets: [capturedBug()] })
    const wrapper = open(transport)
    useTicketDialogStore().openEdit({ projectId: 4, ticketId: 2 })
    await flushPromises()

    const severity = wrapper.find('[data-testid="bug-qualify-severity"]')
      .element as HTMLSelectElement
    expect(severity.selectedOptions[0]?.textContent?.trim()).toBe('Choose a severity')
    expect(severity.selectedOptions[0]?.disabled).toBe(true)
    const submit = wrapper.find('[data-testid="bug-qualify"]').element as HTMLButtonElement
    expect(submit.disabled).toBe(true)

    await wrapper.find('[data-testid="bug-qualify"]').trigger('submit')
    await flushPromises()
    expect(requests('ticket.bug.qualify')).toHaveLength(0)
    expect(wrapper.find('[data-testid="bug-qualify-incomplete"]').text()).toContain('severity')
  })

  it('a Bug edit records the vendor-neutral facts at the read version', async () => {
    const { transport, requests } = editorHarness({ tickets: [capturedBug()] })
    const wrapper = open(transport)
    useTicketDialogStore().openEdit({ projectId: 4, ticketId: 2 })
    await flushPromises()

    await wrapper
      .find('[data-testid="bug-facts-reference-uri-0"]')
      .setValue('https://example.invalid/issues/12')
    await wrapper.find('[data-testid="bug-facts-reference-label-0"]').setValue('The report')
    await wrapper.find('[data-testid="bug-facts-snapshot-at-0"]').setValue('2026-09-05T07:41:00Z')
    await wrapper
      .find('[data-testid="bug-facts-snapshot-observation-0"]')
      .setValue('The log shows the drop.')
    await wrapper.find('[data-testid="bug-facts-evidence"]').setValue('3, 7')
    await wrapper.find('[data-testid="bug-facts"]').trigger('submit')
    await flushPromises()

    expect(requests('ticket.bug.facts')[0]).toEqual({
      mutation: { optimistic_version: 1, idempotency_key: expect.any(String) },
      ticket_id: 2,
      external_references: [{ uri: 'https://example.invalid/issues/12', label: 'The report' }],
      occurrence_snapshots: [
        { observed_at: '2026-09-05T07:41:00Z', observation: 'The log shows the drop.' },
      ],
      evidence_ids: [3, 7],
    })
  })

  it('never aims a qualification at a Bug the dialog was not opened on', async () => {
    const { transport, requests } = editorHarness({
      projects: [editorProject, otherProject],
      specs: [editorSpec],
      tickets: [capturedBug()],
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

  it('fixes a revision to the Ticket record\u2019s own Project', async () => {
    const { transport, requests } = editorHarness({
      projects: [editorProject, otherProject],
      tickets: [qualifiedBug()],
    })
    const wrapper = open(transport)
    useTicketDialogStore().openEdit({ projectId: 4, ticketId: 19 })
    await flushPromises()

    const picker = wrapper.find('[data-testid="ticket-project"]').element as HTMLSelectElement
    expect(picker.disabled).toBe(true)
    expect(picker.value).toBe('4')

    await wrapper.find('[data-testid="bug-qualify"]').trigger('submit')
    await flushPromises()
    expect(requests('ticket.bug.qualify')).toEqual([
      expect.objectContaining({ ticket_id: 19 }),
    ])
  })

  it('closing the dialog leaves the record untouched', async () => {
    const { transport, operations } = editorHarness({ tickets: [implementationTicket()] })
    const wrapper = open(transport)
    useTicketDialogStore().openEdit({ projectId: 4, ticketId: 1 })
    await flushPromises()

    await wrapper.find('[data-testid="ticket-slice"]').setValue('Abandoned')
    await wrapper.find('[data-testid="ticket-dialog-close"]').trigger('click')
    await flushPromises()

    expect(wrapper.find('[data-testid="ticket-dialog"]').exists()).toBe(false)
    expect(operations.filter((entry) => entry.kind === 'command')).toEqual([])
  })
})
