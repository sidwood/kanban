import { mount, flushPromises } from '@vue/test-utils'
import { createPinia } from 'pinia'
import { describe, expect, it, vi } from 'vitest'
import type { InitiativeListResponse, InitiativeRecord } from '@kanban/contracts'
import router from '../router'
import { kanbanTransportKey } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import { useInitiativesStore } from '../stores/initiatives'
import InitiativesView from './InitiativesView.vue'

function record(overrides: Partial<InitiativeRecord> = {}): InitiativeRecord {
  return {
    id: 1,
    name: 'Alpha',
    archived: false,
    version: 1,
    ...overrides,
  }
}

// The listing every mount loads, steerable per test.
function harness(initiatives: InitiativeRecord[]) {
  const query = vi.fn(() =>
    Promise.resolve({ initiatives } satisfies InitiativeListResponse),
  )
  const command = vi.fn(() => Promise.resolve(record()))
  const transport = {
    query,
    command,
    subscribe: () => () => undefined,
    onConnectionChange: () => () => undefined,
  } as unknown as ShellTransport
  return { transport, query, command }
}

async function mounted(initiatives: InitiativeRecord[]) {
  const { transport, query, command } = harness(initiatives)
  router.push('/initiatives')
  await router.isReady()
  const wrapper = mount(InitiativesView, {
    global: {
      plugins: [createPinia(), router],
      provide: { [kanbanTransportKey as symbol]: transport },
    },
  })
  await flushPromises()
  return { wrapper, transport, query, command, store: useInitiativesStore() }
}

describe('InitiativesView', () => {
  it('lists every Initiative, archived ones marked', async () => {
    const { wrapper } = await mounted([
      record(),
      record({ id: 2, name: 'Beta', archived: true, version: 2 }),
    ])

    expect(wrapper.find('[data-testid="initiative-list"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="initiative-name-1"]').text()).toBe('Alpha')
    expect(wrapper.find('[data-testid="initiative-name-2"]').text()).toBe('Beta')
    expect(wrapper.find('[data-testid="initiative-archived-2"]').text()).toBe('Archived')
    expect(wrapper.find('[data-testid="initiative-archived-1"]').exists()).toBe(false)
  })

  it('offers no mutation controls for archived Initiatives and no delete anywhere', async () => {
    const { wrapper } = await mounted([record({ archived: true, version: 2 })])

    expect(wrapper.find('[data-testid="initiative-rename-1"]').exists()).toBe(false)
    expect(wrapper.find('[data-testid="initiative-archive-1"]').exists()).toBe(false)
    const html = wrapper.html()
    expect(html.toLowerCase()).not.toContain('delete')
  })

  it('creating submits a named Initiative through the store', async () => {
    const { wrapper, command } = await mounted([])

    await wrapper.find('[data-testid="initiative-new-name"]').setValue('Reliability')
    await wrapper.find('[data-testid="initiative-create"]').trigger('submit')

    expect(command).toHaveBeenCalledWith(
      'initiative.create',
      expect.objectContaining({ name: 'Reliability' }),
    )
  })

  it('renaming submits through the store with the record identity', async () => {
    const { wrapper, command } = await mounted([record({ id: 4, name: 'Alpha' })])

    await wrapper.find('[data-testid="initiative-rename-4"]').setValue('Beta')
    await wrapper.find('[data-testid="initiative-rename-submit-4"]').trigger('click')

    expect(command).toHaveBeenCalledWith(
      'initiative.rename',
      expect.objectContaining({ initiative_id: 4, name: 'Beta' }),
    )
  })

  it('archiving submits through the store', async () => {
    const { wrapper, command } = await mounted([record({ id: 4, name: 'Alpha' })])

    await wrapper.find('[data-testid="initiative-archive-4"]').trigger('click')

    expect(command).toHaveBeenCalledWith(
      'initiative.archive',
      expect.objectContaining({ initiative_id: 4 }),
    )
  })

  it('shows the store error when a command is refused', async () => {
    const { wrapper, command, store } = await mounted([record()])
    command.mockRejectedValue({
      code: 'invalid_request',
      message: 'archived is terminal; the Initiative accepts no further changes',
    })

    await wrapper.find('[data-testid="initiative-archive-1"]').trigger('click')
    await flushPromises()

    expect(store.error).toContain('terminal')
    expect(wrapper.find('[data-testid="initiative-error"]').text()).toContain('terminal')
  })
})
