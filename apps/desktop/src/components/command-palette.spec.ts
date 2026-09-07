import { mount } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { defineComponent, nextTick } from 'vue'
import type { SearchGlobalResponse } from '@kanban/contracts'
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
            identifier: 'CORE-T1',
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
    expect(wrapper.text()).toContain('CORE-T1')
  })
})
