// KAN-T139-AC1 and KAN-T139-AC3: the one kind-adaptive Ticket editor
// is a dialog, opened from New Ticket, from an uncovered Story, and
// from drawer Edit, preset to the right kind; its fields type,
// validate, save through production commands, surface refusals and
// stale versions, and survive a reload because the record, not a
// client draft, is what a fresh open reads.
import { flushPromises, mount } from '@vue/test-utils'
import type { VueWrapper } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
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
      plugins: [createPinia()],
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

  it('New Ticket opens it on the named Project with every kind offered', async () => {
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
    const kind = wrapper.find('[data-testid="ticket-kind"]').element as HTMLSelectElement
    expect(kind.disabled).toBe(false)
    expect([...kind.options].map((option) => option.value)).toEqual([
      'implementation',
      'bug',
      'task',
    ])
  })

  it('an uncovered Story opens it preset to an Implementation on that Spec and story', async () => {
    const { transport } = editorHarness()
    const wrapper = open(transport)
    useTicketDialogStore().openForStory({ projectId: 4, specId: 7, story: 'CORE-S1-US1' })
    await flushPromises()

    const kind = wrapper.find('[data-testid="ticket-kind"]').element as HTMLSelectElement
    expect(kind.value).toBe('implementation')
    expect(kind.disabled).toBe(true)
    expect((wrapper.find('[data-testid="ticket-spec"]').element as HTMLSelectElement).value).toBe(
      '7',
    )
    expect(
      (wrapper.find('[data-testid="ticket-criterion-stories-0"]').element as HTMLInputElement)
        .value,
    ).toBe('CORE-S1-US1')
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

  it('the form follows the picked kind', async () => {
    const { transport } = editorHarness()
    const wrapper = open(transport)
    useTicketDialogStore().openCreate({ projectId: 4 })
    await flushPromises()

    await wrapper.find('[data-testid="ticket-kind"]').setValue('implementation')
    expect(wrapper.find('[data-testid="ticket-slice"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="ticket-criteria"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="ticket-title"]').exists()).toBe(false)
    expect(wrapper.find('[data-testid="ticket-completion"]').exists()).toBe(false)

    await wrapper.find('[data-testid="ticket-kind"]').setValue('bug')
    expect(wrapper.find('[data-testid="ticket-title"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="ticket-bug-actual"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="ticket-bug-evidence"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="ticket-slice"]').exists()).toBe(false)

    await wrapper.find('[data-testid="ticket-kind"]').setValue('task')
    expect(wrapper.find('[data-testid="ticket-subtype"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="ticket-mode"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="ticket-completion"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="ticket-criteria"]').exists()).toBe(false)
  })

  it('creating an Implementation sends the kind fields to the picked Project', async () => {
    const { transport, requests } = editorHarness()
    const wrapper = open(transport)
    useTicketDialogStore().openCreate({ projectId: 4 })
    await flushPromises()

    await wrapper.find('[data-testid="ticket-kind"]').setValue('implementation')
    await wrapper.find('[data-testid="ticket-spec"]').setValue('7')
    await wrapper.find('[data-testid="ticket-slice"]').setValue('Registration lands end to end')
    await wrapper
      .find('[data-testid="ticket-criterion-outcome-0"]')
      .setValue('Projects register with unique codes.')
    await wrapper.find('[data-testid="ticket-criterion-stories-0"]').setValue('CORE-S1-US1')
    await wrapper.find('[data-testid="ticket-priority"]').setValue('high')
    await wrapper.find('[data-testid="ticket-create"]').trigger('submit')
    await flushPromises()

    expect(requests('ticket.create')[0]).toMatchObject({
      project_id: 4,
      kind: 'implementation',
      priority: 'high',
      spec_id: 7,
      slice: 'Registration lands end to end',
      criteria: [{ outcome: 'Projects register with unique codes.', stories: ['CORE-S1-US1'] }],
    })
    expect(useTicketDialogStore().editorOpen).toBe(false)
  })

  it('creating a Task sends its bounded fields and never story-linked criteria', async () => {
    const { transport, requests } = editorHarness()
    const wrapper = open(transport)
    useTicketDialogStore().openCreate({ projectId: 4 })
    await flushPromises()

    await wrapper.find('[data-testid="ticket-kind"]').setValue('task')
    await wrapper.find('[data-testid="ticket-title"]').setValue('Archive the old register')
    await wrapper.find('[data-testid="ticket-subtype"]').setValue('migration')
    await wrapper.find('[data-testid="ticket-mode"]').setValue('agent')
    await wrapper.find('[data-testid="ticket-completion-outcome-0"]').setValue('The register moves.')
    await wrapper.find('[data-testid="ticket-create"]').trigger('submit')
    await flushPromises()

    const created = requests('ticket.create')[0]
    expect(created).toMatchObject({
      project_id: 4,
      kind: 'task',
      title: 'Archive the old register',
      subtype: 'migration',
      mode: 'agent',
      completion: ['The register moves.'],
    })
    expect(created).not.toHaveProperty('criteria')
    expect(created).not.toHaveProperty('slice')
  })

  it('a refused creation reports the core message and keeps the dialog open', async () => {
    const { transport } = editorHarness({
      answer: (name) =>
        name === 'ticket.create'
          ? Promise.reject({
              code: 'invalid_request',
              message: 'an Implementation Ticket carries story-linked criteria',
            })
          : undefined,
    })
    const wrapper = open(transport)
    useTicketDialogStore().openCreate({ projectId: 4 })
    await flushPromises()

    await wrapper.find('[data-testid="ticket-kind"]').setValue('implementation')
    await wrapper.find('[data-testid="ticket-slice"]').setValue('A slice')
    await wrapper.find('[data-testid="ticket-criterion-outcome-0"]').setValue('An outcome.')
    await wrapper.find('[data-testid="ticket-create"]').trigger('submit')
    await flushPromises()

    expect(wrapper.find('[data-testid="ticket-error"]').text()).toBe(
      'an Implementation Ticket carries story-linked criteria',
    )
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

    // Nothing stands on a quick-captured Bug, so the form offers no
    // severity the operator did not choose (DR-LC-13).
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

  // The T140 review's separate P1 against this ticket: a qualification
  // sent to the Bug a Project switch left behind. The editor cannot
  // reach a Bug it was not opened on — creating offers no
  // qualification at all, and a revision's Project is the record's
  // own fact.
  it('never aims a qualification at a Bug the dialog was not opened on', async () => {
    const { transport, requests } = editorHarness({
      projects: [editorProject, otherProject],
      specs: [editorSpec],
      tickets: [capturedBug()],
    })
    const wrapper = open(transport)
    useTicketDialogStore().openCreate({ projectId: 4 })
    await flushPromises()

    // Quick capture and qualification are separate acts (DR-TK-08,
    // DR-TK-09): creating a Bug offers no qualification to send.
    await wrapper.find('[data-testid="ticket-kind"]').setValue('bug')
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
