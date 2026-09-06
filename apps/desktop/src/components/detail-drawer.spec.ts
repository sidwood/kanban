import { mount } from '@vue/test-utils'
import { describe, expect, it } from 'vitest'
import DetailDrawer from './DetailDrawer.vue'

function openDrawer() {
  document.body.innerHTML = ''
  const wrapper = mount(DetailDrawer, {
    props: { open: true, title: 'Archive the old exports', number: 'KAN-T12' },
    attachTo: document.body,
  })
  return wrapper
}

describe('DetailDrawer', () => {
  it('renders nothing until it opens', () => {
    document.body.innerHTML = ''
    const wrapper = mount(DetailDrawer, {
      props: { open: false, title: 'Hidden' },
      attachTo: document.body,
    })

    expect(document.querySelector('[data-testid="detail-drawer-backdrop"]')).toBeNull()
    expect(document.querySelector('[role="dialog"]')).toBeNull()
    wrapper.unmount()
  })

  it('opens as a labelled modal dialog carrying the record number', () => {
    const wrapper = openDrawer()
    const dialog = document.querySelector('[role="dialog"]')

    expect(dialog?.getAttribute('aria-modal')).toBe('true')
    const heading = document.querySelector('[role="dialog"] h2')
    expect(heading?.getAttribute('id')).toBe(dialog?.getAttribute('aria-labelledby'))
    expect(heading?.textContent).toContain('KAN-T12')
    expect(heading?.textContent).toContain('Archive the old exports')
    expect(document.querySelector('[data-testid="record-number"]')?.textContent).toBe(
      'KAN-T12',
    )
    wrapper.unmount()
  })

  it('closes on the close control and on Escape', async () => {
    const wrapper = openDrawer()

    ;(document.querySelector('[aria-label="Close panel"]') as HTMLElement).click()
    await Promise.resolve()
    expect(wrapper.emitted('close')).toHaveLength(1)

    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }))
    await Promise.resolve()
    expect(wrapper.emitted('close')).toHaveLength(2)
    wrapper.unmount()
  })

  it('traps Tab inside the open panel', async () => {
    const wrapper = openDrawer()
    const panel = document.querySelector('[role="dialog"]') as HTMLElement
    const close = panel.querySelector(
      'button[aria-label="Close panel"]',
    ) as HTMLElement
    close.focus()
    expect(document.activeElement).toBe(close)

    // Tab from the last focusable element wraps to the first.
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Tab' }))
    await Promise.resolve()
    const focusable = panel.querySelectorAll('button')
    expect(document.activeElement).toBe(focusable[0])
    wrapper.unmount()
  })
})
