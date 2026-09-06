const FOCUSABLE_SELECTOR = [
  'a[href]',
  'button:not([disabled])',
  'input:not([disabled]):not([type="hidden"])',
  'select:not([disabled])',
  'textarea:not([disabled])',
  '[tabindex]:not([tabindex="-1"])',
].join(',')

export function getFocusableElements(root: HTMLElement): HTMLElement[] {
  return [...root.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR)].filter(
    (element) =>
      !element.hasAttribute('disabled') &&
      element.getAttribute('aria-hidden') !== 'true' &&
      element.tabIndex !== -1,
  )
}

export function trapTabKey(event: KeyboardEvent, root: HTMLElement): void {
  if (event.key !== 'Tab') return

  const focusable = getFocusableElements(root)
  if (focusable.length === 0) {
    event.preventDefault()
    return
  }

  const first = focusable[0]
  const last = focusable[focusable.length - 1]
  if (first === undefined || last === undefined) {
    event.preventDefault()
    return
  }
  const active = document.activeElement

  if (event.shiftKey) {
    if (active === first || !root.contains(active)) {
      event.preventDefault()
      last.focus()
    }
    return
  }

  if (active === last || !root.contains(active)) {
    event.preventDefault()
    first.focus()
  }
}
