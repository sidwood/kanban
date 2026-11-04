import { mount } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { defineComponent, nextTick } from 'vue'
import { createMemoryHistory, createRouter } from 'vue-router'
import type { SearchGlobalHit, SearchGlobalResponse } from '@kanban/contracts'
import CommandPalette from './CommandPalette.vue'
import { kanbanTransportKey } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import { usePaletteStore } from '../stores/palette'

function harness(answer: (request: unknown) => Promise<SearchGlobalResponse>) {
  const queries: Array<{ name: string; request: unknown }> = []
  const commands: Array<{ name: string; request: unknown }> = []
  const transport = {
    query: (name: string, request: unknown) => {
      queries.push({ name, request })
      return answer(request)
    },
    command: (name: string, request: unknown) => {
      commands.push({ name, request })
      return Promise.reject(new Error('commands are forbidden here'))
    },
    subscribe: () => () => undefined,
  } as unknown as ShellTransport
  return { transport, queries, commands }
}

const RouterStub = defineComponent({
  template: '<div />',
})

describe('CommandPalette', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
  })

  it('searches through the palette without issuing commands', async () => {
    const { transport, queries, commands } = harness(() =>
      Promise.resolve({
        hits: [
          {
            kind: 'ticket',
            id: 1,
            identifier: 'CORE-1',
            label: 'Archive the register',
            project_id: 1,
          },
        ],
      }),
    )
    const palette = usePaletteStore()
    palette.openPalette()

    const wrapper = mount(CommandPalette, {
      global: {
        provide: { [kanbanTransportKey as symbol]: transport },
        stubs: { RouterLink: RouterStub },
      },
    })

    await wrapper.get('[data-testid="palette-query"]').setValue('archive')
    await nextTick()
    await vi.waitFor(() => {
      expect(wrapper.findAll('[data-testid="palette-item"]').length).toBeGreaterThan(0)
    })

    expect(queries).toEqual([{ name: 'search.global', request: { q: 'archive' } }])
    expect(commands).toEqual([])
    expect(wrapper.text()).toContain('CORE-1')
  })

  it('chooses nothing on Enter from a search the input has already left behind', async () => {
    vi.useFakeTimers()
    try {
      const answers: Array<(response: SearchGlobalResponse) => void> = []
      const { transport } = harness(
        () => new Promise<SearchGlobalResponse>((resolve) => answers.push(resolve)),
      )
      const router = createRouter({
        history: createMemoryHistory(),
        routes: [{ path: '/:anywhere(.*)*', component: RouterStub }],
      })
      const push = vi.spyOn(router, 'push')
      const wrapper = mount(CommandPalette, {
        attachTo: document.body,
        global: {
          plugins: [router],
          provide: { [kanbanTransportKey as symbol]: transport },
        },
      })
      usePaletteStore().openPalette()
      await nextTick()
      const input = wrapper.get('[data-testid="palette-query"]')
      const enter = () =>
        window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter' }))

      await input.setValue('arch')
      await vi.advanceTimersByTimeAsync(200)
      await input.setValue('archive')
      const superseded: SearchGlobalHit = {
        kind: 'project',
        id: 4,
        identifier: 'ARCH',
        label: 'Architecture notes',
        project_id: 4,
      }
      answers[0]({ hits: [superseded] })
      await vi.advanceTimersByTimeAsync(50)

      expect(wrapper.text()).not.toContain('ARCH')
      enter()
      await vi.advanceTimersByTimeAsync(0)
      expect(push).not.toHaveBeenCalled()

      await vi.advanceTimersByTimeAsync(150)
      answers[1]({
        hits: [
          { kind: 'ticket', id: 2, identifier: 'CORE-2', label: 'Archive the register', project_id: 1 },
        ],
      })
      await vi.advanceTimersByTimeAsync(0)
      enter()
      await vi.advanceTimersByTimeAsync(0)
      expect(push).toHaveBeenCalledWith('/projects/1/board?ticket=2')
      wrapper.unmount()
    } finally {
      vi.useRealTimers()
    }
  })

  it('chooses nothing on Enter from hits loaded before the input changed', async () => {
    vi.useFakeTimers()
    try {
      const answers: Array<(response: SearchGlobalResponse) => void> = []
      const { transport, queries } = harness(
        () => new Promise<SearchGlobalResponse>((resolve) => answers.push(resolve)),
      )
      const router = createRouter({
        history: createMemoryHistory(),
        routes: [{ path: '/:anywhere(.*)*', component: RouterStub }],
      })
      const push = vi.spyOn(router, 'push')
      const wrapper = mount(CommandPalette, {
        attachTo: document.body,
        global: {
          plugins: [router],
          provide: { [kanbanTransportKey as symbol]: transport },
        },
      })
      usePaletteStore().openPalette()
      await nextTick()
      const input = wrapper.get('[data-testid="palette-query"]')
      const enter = () =>
        window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter' }))

      await input.setValue('arch')
      await vi.advanceTimersByTimeAsync(200)
      answers[0]({
        hits: [
          { kind: 'project', id: 4, identifier: 'ARCH', label: 'Architecture notes', project_id: 4 },
        ],
      })
      await vi.advanceTimersByTimeAsync(0)
      expect(wrapper.text()).toContain('ARCH')

      await input.setValue('archive')
      expect(wrapper.text()).not.toContain('ARCH')
      enter()
      await vi.advanceTimersByTimeAsync(0)
      expect(push).not.toHaveBeenCalled()

      await vi.advanceTimersByTimeAsync(200)
      expect(queries.at(-1)).toEqual({ name: 'search.global', request: { q: 'archive' } })
      enter()
      await vi.advanceTimersByTimeAsync(0)
      expect(push).not.toHaveBeenCalled()
      wrapper.unmount()
    } finally {
      vi.useRealTimers()
    }
  })
})
