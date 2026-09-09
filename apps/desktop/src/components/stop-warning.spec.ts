import { createPinia } from 'pinia'
import { flushPromises, mount } from '@vue/test-utils'
import { describe, expect, it, vi } from 'vitest'
import ServiceLifecycle from './ServiceLifecycle.vue'
import { kanbanTransportKey } from '../core/transport'
import type { LoginLaunchState, ServiceStopWarning } from '@kanban/contracts'

const warning: ServiceStopWarning = { instance_id: 'instance', version: 0, warning_id: 'current-health', capabilities: ['MCP fixture capability', 'Herdr fixture observation'] }
const registration: LoginLaunchState = { instance_id: 'instance', version: 0, enabled: false, error: null }
function harness() {
  const query = vi.fn(async (name: string): Promise<unknown> => name === 'service.stop_warning' ? warning : registration)
  const command = vi.fn(async (): Promise<unknown> => ({ instance_id: 'instance', status: 'stop_requested' }))
  const transport = { query, command, subscribe: () => () => undefined, onConnectionChange: () => () => undefined }
  const wrapper = mount(ServiceLifecycle, { global: { plugins: [createPinia()], provide: { [kanbanTransportKey as symbol]: transport } } })
  return { wrapper, query, command }
}
describe('stop-warning', () => {

  it('discards cancelled and out-of-order warnings before a new confirmation', async () => {
    const { wrapper, query, command } = harness()
    await flushPromises()
    let resolveOld!: (value: unknown) => void
    query.mockImplementationOnce(() => new Promise(resolve => { resolveOld = resolve }))
    await wrapper.get('[data-testid="review-stop"]').trigger('click')
    await wrapper.get('[data-testid="cancel-stop"]').trigger('click')
    query.mockResolvedValueOnce({ ...warning, warning_id: 'new-warning', capabilities: ['New capability'] })
    await wrapper.get('[data-testid="review-stop"]').trigger('click')
    await flushPromises()
    resolveOld(warning)
    await flushPromises()
    expect(wrapper.text()).toContain('New capability')
    expect(wrapper.text()).not.toContain('MCP fixture capability')
    await wrapper.get('[data-testid="confirm-stop"]').trigger('click')
    await flushPromises()
    expect(command).toHaveBeenCalledWith('service.stop', expect.objectContaining({ warning_id: 'new-warning' }))
  })
  it.each(['the stop warning is stale', 'connection ended before the response'])('does not claim stopped on %s', async (message) => {
    const { wrapper, command } = harness()
    await flushPromises()
    command.mockRejectedValueOnce({ code: 'invalid_request', message })
    await wrapper.get('[data-testid="review-stop"]').trigger('click')
    await flushPromises()
    await wrapper.get('[data-testid="confirm-stop"]').trigger('click')
    await flushPromises()
    expect(wrapper.text()).toContain('Service state is unknown')
    expect(wrapper.text()).not.toContain('Stop requested')
    expect(wrapper.find('[data-testid="confirm-stop"]').exists()).toBe(false)
  })
  it('shows no login success until OS readback and reports registration failure', async () => {
    const { wrapper, query, command } = harness()
    await flushPromises()
    let readback!: (value: unknown) => void
    command.mockResolvedValueOnce({ status: 'change_requested' })
    query.mockImplementationOnce(() => new Promise(resolve => { readback = resolve }))
    await wrapper.get('[data-testid="login-enable"]').trigger('click')
    await flushPromises()
    expect(wrapper.get('[data-testid="login-state"]').text()).toContain('Disabled')
    readback({ ...registration, version: 1, enabled: null, error: 'OS readback failed' })
    await flushPromises()
    expect(wrapper.get('[data-testid="login-state"]').text()).toContain('Unknown')
    expect(wrapper.text()).toContain('OS readback failed')
  })

  it('renders Core capabilities, cancels without mutation, and requires a deliberate confirmation', async () => {
    const { wrapper, query, command } = harness()
    await flushPromises()
    await wrapper.get('[data-testid="review-stop"]').trigger('click')
    await flushPromises()
    expect(query).toHaveBeenCalledWith('service.stop_warning', {})
    expect(wrapper.text()).toContain('MCP fixture capability')
    expect(command).not.toHaveBeenCalled()
    await wrapper.get('[data-testid="cancel-stop"]').trigger('click')
    expect(wrapper.find('[role="alertdialog"]').exists()).toBe(false)
    expect(command).not.toHaveBeenCalled()
    await wrapper.get('[data-testid="review-stop"]').trigger('click')
    await flushPromises()
    await wrapper.get('[data-testid="confirm-stop"]').trigger('click')
    await flushPromises()
    expect(command).toHaveBeenCalledWith('service.stop', { mutation: { optimistic_version: 0, idempotency_key: expect.any(String) }, instance_id: 'instance', warning_id: 'current-health', confirmed: true })
    expect(wrapper.text()).toContain('Stop requested')
    expect(wrapper.text()).not.toContain('Service stopped')
  })
})
