import { mount } from '@vue/test-utils'
import type { VueWrapper } from '@vue/test-utils'
import { afterEach, describe, expect, it } from 'vitest'
import type { CardChip } from '../../views/board-chips'
import BoardCard from './BoardCard.vue'

const mounted: VueWrapper[] = []

function mountCard(chips: readonly CardChip[] = []) {
  const wrapper = mount(BoardCard, {
    props: {
      ticketId: 7,
      number: 'CORE-12',
      title: 'Archive the old exports',
      kindLabel: '',
      projectCode: 'CORE',
      showProject: false,
      chips,
      statusLabel: 'Ready',
      statusTone: 'neutral',
      showsStatus: false,
      draggable: true,
      dragging: false,
      chrome: 'border-line',
      label: 'Archive the old exports',
      kind: 'task',
      state: 'ready',
    },
    attachTo: document.body,
  })
  mounted.push(wrapper)
  return wrapper
}

afterEach(() => {
  for (const wrapper of mounted.splice(0)) wrapper.unmount()
})

describe('BoardCard opening', () => {
  it('opens from a focusable title button, once per activation', () => {
    const wrapper = mountCard()

    const title = wrapper.find('button[data-testid="open-ticket-7"]')
    expect(title.exists()).toBe(true)
    expect(title.attributes('type')).toBe('button')
    expect(title.text()).toBe('Archive the old exports')
    expect(title.classes()).toEqual(expect.arrayContaining(['text-sm', 'font-medium', 'text-ink']))

    const button = title.element as HTMLButtonElement
    button.focus()
    expect(document.activeElement).toBe(button)
    expect(button.tabIndex).toBe(0)

    // A browser turns Enter or Space on a focused button into a click.
    button.click()

    expect(wrapper.emitted('open')).toHaveLength(1)
  })

  it('still opens from a pointer click anywhere on the card', async () => {
    const wrapper = mountCard()

    await wrapper.find('[data-testid="card-number-7"]').trigger('click')

    expect(wrapper.emitted('open')).toHaveLength(1)
  })
})
