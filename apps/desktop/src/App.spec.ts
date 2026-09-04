import { mount } from '@vue/test-utils'
import { createPinia } from 'pinia'
import { describe, expect, it, vi } from 'vitest'
import router from './router'
import { kanbanTransportKey } from './core/transport'
import type { ShellTransport } from './core/transport'
import App from './App.vue'

// The app shell mounts the boot surface against a provided
// transport; the real one is the Tauri bridge in main.ts.
const transport = {
  query: vi.fn(() => new Promise(() => undefined)),
  command: vi.fn(),
  subscribe: () => () => undefined,
  onConnectionChange: () => () => undefined,
} as unknown as ShellTransport

describe('App', () => {
  it('renders the application shell and its boot surface', async () => {
    router.push('/')
    await router.isReady()
    const wrapper = mount(App, {
      global: {
        plugins: [createPinia(), router],
        provide: { [kanbanTransportKey as symbol]: transport },
      },
    })
    expect(wrapper.find('h1').text()).toBe('Kanban')
    expect(wrapper.find('[data-testid="connection-status"]').exists()).toBe(true)
  })
})
