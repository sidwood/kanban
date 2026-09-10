import { flushPromises, mount } from '@vue/test-utils'
import { createPinia } from 'pinia'
import { describe, expect, it } from 'vitest'
import router from '../router'
import { kanbanTransportKey } from '../core/transport'
import { harness } from '../test/shell-harness'
import WorkspacesIndexView from './WorkspacesIndexView.vue'

describe('the Workspaces index', () => {
  it('lists every Project and links to its Workspaces and Lanes', async () => {
    await router.push('/workspaces')
    const wrapper = mount(WorkspacesIndexView, {
      global: {
        plugins: [createPinia(), router],
        provide: { [kanbanTransportKey as symbol]: harness().transport },
      },
    })
    await flushPromises()

    expect(wrapper.get('[data-testid="workspaces-project-1"]').attributes('href')).toBe(
      '/projects/1/workspaces',
    )
    expect(wrapper.get('[data-testid="workspaces-project-2"]').text()).toContain('EDGE')
  })

  it('points at registration when no Project stands', async () => {
    await router.push('/workspaces')
    const wrapper = mount(WorkspacesIndexView, {
      global: {
        plugins: [createPinia(), router],
        provide: { [kanbanTransportKey as symbol]: harness({ projects: [] }).transport },
      },
    })
    await flushPromises()

    expect(wrapper.text()).toContain('No Project is registered')
    expect(wrapper.get('[data-testid="workspaces-register"]').attributes('href')).toBe('/register')
  })
})
