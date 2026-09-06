import { mount } from '@vue/test-utils'
import { describe, expect, it } from 'vitest'
import StatusBadge from './StatusBadge.vue'

describe('StatusBadge', () => {
  it('renders its label in the tone it is given', () => {
    const wrapper = mount(StatusBadge, { props: { tone: 'positive' }, slots: { default: 'Active' } })

    expect(wrapper.text()).toBe('Active')
    expect(wrapper.attributes('data-tone')).toBe('positive')
    expect(wrapper.classes()).toContain('bg-accent/12')
    expect(wrapper.classes()).toContain('text-accent')
  })

  it('carries every tone the board vocabulary spends', () => {
    for (const tone of ['neutral', 'progress', 'caution', 'critical'] as const) {
      const wrapper = mount(StatusBadge, { props: { tone } })
      expect(wrapper.attributes('data-tone'), tone).toBe(tone)
    }
  })

  it('announces itself as an uppercase label with its dot', () => {
    const wrapper = mount(StatusBadge, {
      props: { tone: 'caution' },
      slots: { default: 'Blocked' },
    })

    expect(wrapper.classes()).toContain('uppercase')
    expect(wrapper.find('span[aria-hidden="true"]').exists()).toBe(true)
  })
})
