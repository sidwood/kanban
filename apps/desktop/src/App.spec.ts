import { flushPromises, mount } from '@vue/test-utils'
import type { VueWrapper } from '@vue/test-utils'
import { createPinia } from 'pinia'
import type { Pinia } from 'pinia'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import router from './router'
import { kanbanTransportKey } from './core/transport'
import { ATTENTION_POLL_MS, useShellStore } from './stores/shell'
import { harness, ticket } from './test/shell-harness'
import App from './App.vue'

// Every mount is taken down again, so one test's shell never keeps
// reacting to the shared router under the next test's feet.
const mounted: VueWrapper[] = []
let pinia: Pinia

async function mountApp(transport: ReturnType<typeof harness>['transport'], path = '/') {
  await router.push(path)
  await router.isReady()
  pinia = createPinia()
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

beforeEach(() => {
  localStorage.clear()
  document.documentElement.classList.remove('dark')
})

afterEach(() => {
  for (const wrapper of mounted.splice(0)) wrapper.unmount()
  document.body.innerHTML = ''
})

describe('the application shell', () => {
  it('opens on the board and wraps every route in the rail and top bar', async () => {
    const wrapper = await mountApp(harness({ tickets: [ticket()] }).transport)

    expect(router.currentRoute.value.path).toBe('/board')
    expect(wrapper.find('[data-testid="app-rail"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="app-top-bar"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="brand"]').text()).toContain('Kanban')
    expect(wrapper.find('[data-testid="kanban-board"]').exists()).toBe(true)

    await router.push('/attention')
    await flushPromises()
    expect(wrapper.find('[data-testid="app-rail"]').exists()).toBe(true)
    expect(wrapper.find('h1').text()).toContain('Attention')
  })

  it('groups the rail as Pipeline, Execution, and Authoring and keeps every surface reachable', async () => {
    const wrapper = await mountApp(harness().transport)
    const rail = wrapper.get('[data-testid="app-rail"]')

    expect(
      rail.findAll('[data-testid^="rail-group-"]').map((group) => group.attributes('data-testid')),
    ).toEqual(['rail-group-pipeline', 'rail-group-execution', 'rail-group-authoring'])
    expect(rail.find('[data-testid="rail-group-pipeline"]').text()).toContain('Pipeline')
    expect(rail.find('[data-testid="rail-group-execution"]').text()).toContain('Execution')
    expect(rail.find('[data-testid="rail-group-authoring"]').text()).toContain('Authoring')
    const links: Record<string, string> = {
      boards: '/board',
      attention: '/attention',
      planning: '/planning',
      activity: '/activity',
      workspaces: '/workspaces',
      profiles: '/settings/profiles',
      projects: '/register',
      initiatives: '/initiatives',
      herdr: '/settings/herdr',
      capacity: '/settings/capacity',
      health: '/health',
    }
    for (const [id, href] of Object.entries(links)) {
      expect(rail.get(`[data-testid="rail-link-${id}"]`).attributes('href'), id).toBe(href)
    }
    const groupOf = (id: string): string | undefined =>
      rail
        .findAll('[data-testid^="rail-group-"]')
        .find((group) => group.find(`[data-testid="rail-link-${id}"]`).exists())
        ?.attributes('data-testid')
    expect(groupOf('boards')).toBe('rail-group-pipeline')
    expect(groupOf('attention')).toBe('rail-group-pipeline')
    expect(groupOf('activity')).toBe('rail-group-pipeline')
    expect(groupOf('workspaces')).toBe('rail-group-execution')
    expect(groupOf('profiles')).toBe('rail-group-execution')
    expect(groupOf('planning')).toBe('rail-group-authoring')
    expect(groupOf('projects')).toBe('rail-group-authoring')
    expect(groupOf('initiatives')).toBe('rail-group-authoring')
  })

  it('carries none of the prototype scaffolding', async () => {
    const wrapper = await mountApp(harness().transport)
    const text = wrapper.text()

    expect(text).not.toContain('States gallery')
    expect(text).not.toContain('Ticket editors')
    expect(text).not.toContain('Prototype')
    expect(text).not.toContain('Sign out')
    // Authoring is a specified rail group; the editor destination it
    // held in the prototype is the excluded part.
    expect(wrapper.find('[data-testid="rail-link-editor"]').exists()).toBe(false)
    expect(wrapper.find('[data-testid="rail-operator"]').text()).toContain('Operator')
    // No lane fraction, no name: nothing the shell cannot vouch for.
    expect(wrapper.find('[data-testid="rail-operator"]').text()).not.toMatch(/\d of \d/)
  })

  it('marks the current surface in the rail', async () => {
    const wrapper = await mountApp(harness().transport, '/settings/profiles')

    expect(wrapper.get('[data-testid="rail-link-profiles"]').attributes('aria-current')).toBe('page')
    expect(wrapper.get('[data-testid="rail-link-boards"]').attributes('aria-current')).toBeUndefined()
  })

  it('collapses and expands the rail when the core is unreachable', async () => {
    const shell = harness()
    shell.command.mockImplementation((name: string) => {
      if (name === 'shell.preferences.update') {
        return Promise.reject({ code: 'unavailable', message: 'the core is unreachable' })
      }
      return Promise.resolve({})
    })
    const wrapper = await mountApp(shell.transport)
    expect(wrapper.get('[data-testid="app-shell"]').attributes('data-rail-open')).toBe('true')

    await wrapper.get('[data-testid="rail-toggle"]').trigger('click')
    await flushPromises()

    expect(wrapper.get('[data-testid="app-shell"]').attributes('data-rail-open')).toBe('false')
    expect(wrapper.get('[data-testid="rail-toggle"]').attributes('aria-expanded')).toBe('false')

    await wrapper.get('[data-testid="rail-toggle"]').trigger('click')
    await flushPromises()

    expect(wrapper.get('[data-testid="app-shell"]').attributes('data-rail-open')).toBe('true')
  })

  it('collapses the rail through the core and keeps the choice across a reload', async () => {
    const shell = harness()
    const wrapper = await mountApp(shell.transport)
    expect(wrapper.get('[data-testid="app-shell"]').attributes('data-rail-open')).toBe('true')

    await wrapper.get('[data-testid="rail-toggle"]').trigger('click')
    await flushPromises()

    expect(wrapper.get('[data-testid="app-shell"]').attributes('data-rail-open')).toBe('false')
    expect(wrapper.get('[data-testid="rail-toggle"]').attributes('aria-expanded')).toBe('false')
    expect(shell.command).toHaveBeenCalledWith(
      'shell.preferences.update',
      expect.objectContaining({ rail_open: false }),
    )
    expect(shell.preferences().rail_open).toBe(false)
    // Nothing of the arrangement is kept in the browser.
    expect(localStorage.length).toBe(0)

    // A fresh mount over the same core is what a reload is.
    const again = await mountApp(shell.transport)
    expect(again.get('[data-testid="app-shell"]').attributes('data-rail-open')).toBe('false')
  })

  it('keeps the icon rail at narrow width and restores the labels when room returns', async () => {
    const wrapper = await mountApp(harness().transport)
    expect(wrapper.get('[data-testid="app-shell"]').attributes('data-narrow')).toBe('false')

    useShellStore(pinia).setNarrow(true)
    await flushPromises()
    expect(wrapper.get('[data-testid="app-shell"]').attributes('data-narrow')).toBe('true')
    expect(wrapper.get('[data-testid="app-shell"]').attributes('data-rail-open')).toBe('false')
    // Every destination is still there, icon-only, and still navigates.
    await wrapper.get('[data-testid="rail-link-planning"]').trigger('click')
    await flushPromises()
    expect(router.currentRoute.value.path).toBe('/planning')

    useShellStore(pinia).setNarrow(false)
    await flushPromises()
    expect(wrapper.get('[data-testid="app-shell"]').attributes('data-rail-open')).toBe('true')
  })

  it('re-reads the inbox count on the inbox\'s own cadence', async () => {
    vi.useFakeTimers()
    try {
      const shell = harness()
      const wrapper = await mountApp(shell.transport)
      const before = shell.query.mock.calls.filter(([name]) => name === 'attention.list').length

      await vi.advanceTimersByTimeAsync(ATTENTION_POLL_MS)

      expect(shell.query.mock.calls.filter(([name]) => name === 'attention.list').length).toBe(
        before + 1,
      )
      expect(wrapper.find('[data-testid="app-rail"]').exists()).toBe(true)
    } finally {
      vi.useRealTimers()
    }
  })

  it('counts the attention items waiting on the operator', async () => {
    const wrapper = await mountApp(
      harness({
        attention: [
          {
            id: 'a1',
            kind: 'blocker',
            project_id: 1,
            subject_kind: 'ticket',
            subject_id: '7',
            summary: 'blocked',
            detail: {},
            active: true,
            first_seen_at: '2026-09-13T10:00:00Z',
            last_seen_at: '2026-09-13T10:00:00Z',
            acknowledged_at: null,
            acknowledged_by: null,
            version: 1,
          },
          {
            id: 'a2',
            kind: 'stale_run',
            project_id: 1,
            subject_kind: 'run',
            subject_id: '3',
            summary: 'stale',
            detail: {},
            active: true,
            first_seen_at: '2026-09-13T10:00:00Z',
            last_seen_at: '2026-09-13T10:00:00Z',
            acknowledged_at: '2026-09-13T11:00:00Z',
            acknowledged_by: 'sid',
            version: 1,
          },
        ],
      }).transport,
    )

    expect(wrapper.get('[data-testid="rail-attention-count"]').text()).toBe('1')
  })

  it('scopes the board to one Project or every Project from the top bar', async () => {
    const wrapper = await mountApp(harness().transport)
    expect(wrapper.get('[data-testid="scope-menu"]').text()).toContain('All projects')

    await wrapper.get('[data-testid="scope-menu"]').trigger('click')
    const menu = wrapper.get('[role="menu"][aria-label="Board scope"]')
    expect(menu.text()).toContain('CORE')
    expect(menu.text()).toContain('EDGE')
    await menu.get('[data-testid="scope-option-2"]').trigger('click')
    await flushPromises()

    expect(router.currentRoute.value.path).toBe('/projects/2/board')
    expect(wrapper.get('[data-testid="scope-menu"]').text()).toContain('EDGE')
    expect(wrapper.find('[role="menu"]').exists()).toBe(false)
    // The scoped Workspaces surface follows the scope.
    expect(wrapper.get('[data-testid="rail-link-workspaces"]').attributes('href')).toBe(
      '/projects/2/workspaces',
    )

    await wrapper.get('[data-testid="scope-menu"]').trigger('click')
    await wrapper.get('[data-testid="scope-option-all"]').trigger('click')
    await flushPromises()
    expect(router.currentRoute.value.path).toBe('/board')
    expect(wrapper.get('[data-testid="rail-link-workspaces"]').attributes('href')).toBe('/workspaces')
  })

  it('adopts the scope a board link arrives on', async () => {
    const wrapper = await mountApp(harness().transport, '/projects/1/board')

    expect(wrapper.get('[data-testid="scope-menu"]').text()).toContain('CORE')
  })

  it('opens the command palette from the search control', async () => {
    const wrapper = await mountApp(harness().transport)
    expect(wrapper.find('[data-testid="command-palette"]').exists()).toBe(false)

    await wrapper.get('[data-testid="open-search"]').trigger('click')

    expect(wrapper.find('[data-testid="command-palette"]').exists()).toBe(true)
  })

  it('shows the observed connection and never toggles it', async () => {
    const shell = harness()
    const wrapper = await mountApp(shell.transport)
    const chip = wrapper.get('[data-testid="connection-chip"]')
    expect(chip.text()).toContain('Service running')
    expect(chip.attributes('data-phase')).toBe('connected')
    expect(chip.attributes('href')).toBe('/health')

    await chip.trigger('click')
    await flushPromises()
    expect(shell.command).not.toHaveBeenCalled()
    expect(wrapper.get('[data-testid="connection-chip"]').attributes('data-phase')).toBe(
      'connected',
    )

    // The shell's announcement re-verifies through the same query.
    shell.query.mockImplementation((name: string) =>
      name === 'health.get'
        ? Promise.reject({ code: 'unavailable', message: 'offline' })
        : Promise.resolve({}),
    )
    shell.connection('disconnected')
    await flushPromises()
    expect(wrapper.get('[data-testid="connection-chip"]').attributes('data-phase')).toBe(
      'disconnected',
    )
    expect(wrapper.get('[data-testid="connection-chip"]').text()).toContain('Core unreachable')
  })

  it('switches the theme from the top bar and keeps it', async () => {
    const wrapper = await mountApp(harness().transport)

    await wrapper.get('[data-testid="theme-toggle"]').trigger('click')
    expect(document.documentElement.classList.contains('dark')).toBe(true)
    expect(localStorage.getItem('kanban.theme.v1')).toBe('dark')

    await wrapper.get('[data-testid="theme-toggle"]').trigger('click')
    expect(document.documentElement.classList.contains('dark')).toBe(false)
  })

  // The editor has no destination at all: New Ticket opens the dialog
  // in place, and the rail offers nothing (KAN-T139-AC1).
  it('reaches the ticket editor from the board as a dialog, not from the rail', async () => {
    const wrapper = await mountApp(harness({ tickets: [ticket()] }).transport)
    expect(wrapper.find('[data-testid="rail-link-tickets"]').exists()).toBe(false)

    const button = wrapper.get('[data-testid="new-ticket"]')
    expect(button.attributes('href')).toBeUndefined()
    expect(wrapper.find('[data-testid="ticket-dialog"]').exists()).toBe(false)
    await button.trigger('click')
    await flushPromises()
    expect(wrapper.find('[data-testid="ticket-dialog"]').exists()).toBe(true)
  })
})
