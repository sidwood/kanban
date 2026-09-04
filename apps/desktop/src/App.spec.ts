import { mount } from '@vue/test-utils'
import { createPinia } from 'pinia'
import { describe, expect, it } from 'vitest'
import router from './router'
import App from './App.vue'

describe('App', () => {
  it('renders the application shell', async () => {
    router.push('/')
    await router.isReady()
    const wrapper = mount(App, {
      global: { plugins: [createPinia(), router] },
    })
    expect(wrapper.find('h1').text()).toBe('Kanban')
  })
})
