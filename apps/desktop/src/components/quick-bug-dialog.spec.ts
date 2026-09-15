// KAN-T139-AC2: quick Bug capture is a small dialog off a global
// shortcut, and it asks for exactly the three facts DR-TK-08 makes a
// capture out of — title, actual behaviour, and reporter evidence.
import { flushPromises, mount } from '@vue/test-utils'
import type { VueWrapper } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { kanbanTransportKey } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import { useTicketDialogStore } from '../stores/ticket-dialog'
import { editorHarness, editorProject, otherProject } from '../test/editor-harness'
import QuickBugDialog from './QuickBugDialog.vue'

const mounted: VueWrapper[] = []

function open(transport: ShellTransport) {
  const wrapper = mount(QuickBugDialog, {
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

describe('quick Bug capture', () => {
  it('stays shut until the shortcut opens it', async () => {
    const wrapper = open(editorHarness().transport)
    await flushPromises()

    expect(wrapper.find('[data-testid="quick-bug-dialog"]').exists()).toBe(false)
  })

  it('is a small dialog carrying exactly the three capture facts', async () => {
    const { transport } = editorHarness({ projects: [editorProject, otherProject] })
    const wrapper = open(transport)
    useTicketDialogStore().openQuickBug({ projectId: 4 })
    await flushPromises()

    const dialog = wrapper.find('[data-testid="quick-bug-dialog"]')
    expect(dialog.exists()).toBe(true)
    expect(dialog.attributes('role')).toBe('dialog')
    expect(dialog.attributes('aria-modal')).toBe('true')
    expect(dialog.attributes('data-dialog-size')).toBe('small')
    expect(wrapper.find('[data-testid="quick-bug-title"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="quick-bug-actual"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="quick-bug-evidence"]').exists()).toBe(true)
    // Nothing a qualification owns belongs to a capture (DR-TK-09).
    expect(wrapper.find('[data-testid="quick-bug-severity"]').exists()).toBe(false)
    expect(wrapper.find('[data-testid="quick-bug-criteria"]').exists()).toBe(false)
  })

  it('will not send a capture missing one of the three facts', async () => {
    const { transport, requests } = editorHarness()
    const wrapper = open(transport)
    useTicketDialogStore().openQuickBug({ projectId: 4 })
    await flushPromises()

    await wrapper.find('[data-testid="quick-bug-title"]').setValue('Landing drops the branch')
    await wrapper.find('[data-testid="quick-bug-actual"]').setValue('The branch is dropped.')
    expect(
      (wrapper.find('[data-testid="quick-bug-capture"]').element as HTMLButtonElement).disabled,
    ).toBe(true)
    await wrapper.find('[data-testid="quick-bug-capture"]').trigger('submit')
    await flushPromises()
    expect(requests('ticket.create')).toHaveLength(0)

    await wrapper.find('[data-testid="quick-bug-evidence"]').setValue('The landing log names it.')
    expect(
      (wrapper.find('[data-testid="quick-bug-capture"]').element as HTMLButtonElement).disabled,
    ).toBe(false)
  })

  it('captures a Bug through ticket.create and closes', async () => {
    const { transport, requests } = editorHarness()
    const wrapper = open(transport)
    useTicketDialogStore().openQuickBug({ projectId: 4 })
    await flushPromises()

    await wrapper.find('[data-testid="quick-bug-title"]').setValue('Landing drops the branch')
    await wrapper.find('[data-testid="quick-bug-actual"]').setValue('The branch is dropped.')
    await wrapper.find('[data-testid="quick-bug-evidence"]').setValue('The landing log names it.')
    await wrapper.find('[data-testid="quick-bug-capture"]').trigger('submit')
    await flushPromises()

    expect(requests('ticket.create')[0]).toMatchObject({
      project_id: 4,
      kind: 'bug',
      title: 'Landing drops the branch',
      actual_behaviour: 'The branch is dropped.',
      reporter_evidence: 'The landing log names it.',
    })
    expect(requests('ticket.create')[0]).not.toHaveProperty('criteria')
    expect(useTicketDialogStore().quickBugOpen).toBe(false)
  })

  it('reports a refused capture and keeps what was typed', async () => {
    const { transport } = editorHarness({
      answer: (name) =>
        name === 'ticket.create'
          ? Promise.reject({ code: 'invalid_request', message: 'a Ticket title cannot be blank' })
          : undefined,
    })
    const wrapper = open(transport)
    useTicketDialogStore().openQuickBug({ projectId: 4 })
    await flushPromises()

    await wrapper.find('[data-testid="quick-bug-title"]').setValue('Landing drops the branch')
    await wrapper.find('[data-testid="quick-bug-actual"]').setValue('The branch is dropped.')
    await wrapper.find('[data-testid="quick-bug-evidence"]').setValue('The landing log names it.')
    await wrapper.find('[data-testid="quick-bug-capture"]').trigger('submit')
    await flushPromises()

    expect(wrapper.find('[data-testid="quick-bug-error"]').text()).toBe(
      'a Ticket title cannot be blank',
    )
    expect(
      (wrapper.find('[data-testid="quick-bug-title"]').element as HTMLInputElement).value,
    ).toBe('Landing drops the branch')
    expect(useTicketDialogStore().quickBugOpen).toBe(true)
  })

  it('asks which Project the capture belongs to when the shortcut names none', async () => {
    const { transport, requests } = editorHarness({ projects: [editorProject, otherProject] })
    const wrapper = open(transport)
    useTicketDialogStore().openQuickBug({ projectId: null })
    await flushPromises()

    const picker = wrapper.find('[data-testid="quick-bug-project"]')
      .element as HTMLSelectElement
    expect(picker.value).toBe('4')
    await wrapper.find('[data-testid="quick-bug-project"]').setValue('5')
    await wrapper.find('[data-testid="quick-bug-title"]').setValue('Edge drops the branch')
    await wrapper.find('[data-testid="quick-bug-actual"]').setValue('The branch is dropped.')
    await wrapper.find('[data-testid="quick-bug-evidence"]').setValue('The log names it.')
    await wrapper.find('[data-testid="quick-bug-capture"]').trigger('submit')
    await flushPromises()

    expect(requests('ticket.create')[0]).toMatchObject({ project_id: 5, kind: 'bug' })
  })
})
