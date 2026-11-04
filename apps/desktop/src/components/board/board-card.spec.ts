import { mount } from '@vue/test-utils'
import type { VueWrapper } from '@vue/test-utils'
import { afterEach, describe, expect, it, vi } from 'vitest'
import type { IssueRecord } from '@kanban/contracts'
import { ticket } from '../../test/shell-harness'
import type { CardChip } from '../../views/board-chips'
import { linearChipsFor } from '../../views/linear-chips'
import BoardCard from './BoardCard.vue'

const mounted: VueWrapper[] = []

function issueWithLabels(labels: string[]): IssueRecord {
  return {
    identifier: 'CORE-12',
    url: 'kanban://issue/CORE-12',
    ticket_id: 7,
    project_id: 1,
    title: 'Archive the old exports',
    description: '',
    state: 'Ready',
    state_type: 'backlog',
    priority: 3,
    labels,
    relations: [],
    state_history: [],
    version: 3,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
  }
}

function chipsWithLabels(labels: string[]): readonly CardChip[] {
  return linearChipsFor(ticket(), { issue: issueWithLabels(labels) })
}

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
  vi.restoreAllMocks()
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

describe('BoardCard label chips', () => {
  it('gives every label chip its own key, the same whichever labels sit beside it', () => {
    const several = chipsWithLabels(['Alpha', 'Beta', 'Gamma'])
    const keys = several.map((chip) => chip.key ?? chip.kind)
    expect(new Set(keys).size).toBe(keys.length)

    const betaKey = (chips: readonly CardChip[]) =>
      chips.find((chip) => chip.label === 'Label' && chip.value === 'Beta')?.key
    expect(betaKey(several)).toBeDefined()
    expect(betaKey(chipsWithLabels(['Beta']))).toBe(betaKey(several))
  })

  it('keeps each label chip on its own element when a sibling label leaves', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => undefined)
    const wrapper = mountCard(chipsWithLabels(['Alpha', 'Beta']))

    const labelChips = () =>
      wrapper.findAll('[data-testid="card-chips-7"] li').filter((chip) => chip.text().startsWith('Label'))
    const beta = labelChips().find((chip) => chip.text().endsWith('Beta'))?.element
    expect(beta).toBeDefined()

    await wrapper.setProps({ chips: chipsWithLabels(['Beta']) })

    expect(labelChips()).toHaveLength(1)
    expect(labelChips()[0]?.element).toBe(beta)
    const duplicateKeyWarnings = warn.mock.calls.filter((call) =>
      String(call[0]).includes('Duplicate keys'),
    )
    expect(duplicateKeyWarnings).toEqual([])
  })
})
