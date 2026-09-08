import { flushPromises, mount } from '@vue/test-utils'
import { afterEach, describe, expect, it } from 'vitest'
import type { NotificationSettingsUpdateRequest, NotificationPermissionRecord, NotificationDeliveryRecord, NotificationRetryRequest, ProjectRecord } from '@kanban/contracts'
import { kanbanTransportKey } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import NotificationSettings from './NotificationSettings.vue'

const project = {
  id: 1, code: 'CORE', name: 'Control plane', repository: '/repositories/kanban',
  seed_workspace: '/workspaces/kanban.seed', default_branch: 'main', herdr_session: 'kanban-main',
  herdr_workspace: 'kanban.seed', initiative_id: null, archived: false,
  counters: { plan: 0, spec: 0, ticket: 1 }, version: 1,
} satisfies ProjectRecord
const mounted: Array<{ unmount(): void }> = []
afterEach(() => { mounted.splice(0).forEach((wrapper) => wrapper.unmount()) })

function harness(native: NotificationPermissionRecord = { state: 'granted', reason: null, request_pending: false }, history: NotificationDeliveryRecord[] = []) {
  const operations: Array<{ kind: string; name: string; request: unknown }> = []
  const answers: Record<string, unknown> = {
    'notification.settings.get': { project_id: 1, local_enabled: false, mirror_role: null, version: 0 },
    'notification.permission.get': native,
    'notification.deliveries': { deliveries: history },
  }
  const transport = {
    query: (name: string, request: unknown) => {
      operations.push({ kind: 'query', name, request })
      return Promise.resolve(answers[name])
    },
    command: (name: string, request: unknown) => {
      operations.push({ kind: 'command', name, request })
      if (name === 'notification.settings.update') {
        const update = request as NotificationSettingsUpdateRequest
        const saved = { project_id: update.project_id, local_enabled: update.local_enabled, mirror_role: update.mirror_role, version: update.mutation.optimistic_version + 1 }
        answers['notification.settings.get'] = saved
        return Promise.resolve(saved)
      }
      if (name === 'notification.permission.request') return Promise.resolve({ accepted: true })
      if (name === 'notification.retry') {
        const retry = request as NotificationRetryRequest
        const current = answers['notification.deliveries'] as { deliveries: NotificationDeliveryRecord[] }
        const retried = current.deliveries.find((delivery) => delivery.id === retry.delivery_id)!
        const result = { ...retried, status: 'queued' as const, version: retried.version + 1 }
        answers['notification.deliveries'] = { deliveries: current.deliveries.map((delivery) => delivery.id === result.id ? result : delivery) }
        return Promise.resolve(result)
      }
      return Promise.reject(new Error(`Unexpected command: ${name}`))
    },
    subscribe: () => () => undefined,
    onConnectionChange: () => () => undefined,
  } as unknown as ShellTransport
  const wrapper = mount(NotificationSettings, { props: { projects: [project] }, global: { provide: { [kanbanTransportKey as symbol]: transport } } })
  mounted.push(wrapper)
  return { wrapper, operations, answers }
}

describe('notification-settings', () => {
  it('saves explicit channel preferences without acknowledging attention', async () => {
    const { wrapper, operations } = harness()
    await flushPromises()
    expect(wrapper.find('[data-testid="notification-local"]').exists()).toBe(true)
    expect(operations.filter((op) => op.kind === 'command')).toEqual([])
    await wrapper.get('[data-testid="notification-local"]').setValue(true)
    await wrapper.get('[data-testid="notification-mirror"]').setValue(true)
    await wrapper.get('[data-testid="notification-role"]').setValue('observer')
    await wrapper.get('[data-testid="notification-settings-form"]').trigger('submit')
    await flushPromises()
    expect(operations.filter((op) => op.kind === 'command')).toEqual([{
      kind: 'command', name: 'notification.settings.update', request: {
        mutation: { optimistic_version: 0, idempotency_key: expect.any(String) },
        project_id: 1, local_enabled: true, mirror_role: 'observer',
      },
    }])
    expect(wrapper.text()).toContain('Preferences saved')
  })

  it('requests permission only on an explicit action and does not invent a grant', async () => {
    const { wrapper, operations } = harness({ state: 'not_determined', reason: null, request_pending: false })
    await flushPromises()
    expect(wrapper.get('[data-testid="notification-local"]').attributes('disabled')).toBeDefined()
    expect(wrapper.find('[data-testid="notification-request-permission"]').exists()).toBe(true)
    expect(operations.filter((op) => op.kind === 'command')).toEqual([])
    await wrapper.get('[data-testid="notification-request-permission"]').trigger('click')
    await flushPromises()
    expect(operations.filter((op) => op.kind === 'command')).toEqual([{
      kind: 'command', name: 'notification.permission.request', request: {
        mutation: { optimistic_version: 0, idempotency_key: expect.any(String) },
      },
    }])
    expect(wrapper.get('[data-testid="notification-permission"]').text()).toContain('not determined')
    expect(wrapper.get('[data-testid="notification-local"]').attributes('disabled')).toBeDefined()
  })


  it('reads a later native grant without treating the permission request as a grant', async () => {
    const { wrapper, operations, answers } = harness({ state: 'not_determined', reason: null, request_pending: false })
    await flushPromises()
    answers['notification.permission.get'] = { state: 'granted', reason: null, request_pending: false }
    expect(wrapper.find('[data-testid="notification-refresh"]').exists()).toBe(true)
    await wrapper.get('[data-testid="notification-refresh"]').trigger('click')
    await flushPromises()
    expect(wrapper.get('[data-testid="notification-local"]').attributes('disabled')).toBeUndefined()
    expect(wrapper.get('[data-testid="notification-permission"]').text()).toContain('granted')
    expect(operations.filter((op) => op.kind === 'command')).toEqual([])
  })


  it('offers retries for known failures but not submitted or uncertain deliveries', async () => {
    const base: NotificationDeliveryRecord = {
      id: 1, project_id: 1, item_id: 'attention-0', item_version: 1, channel: 'local', target: { kind: 'local' },
      status: 'failed', receipt: null, last_error: 'Permission was unavailable',
      created_at: '2026-09-08T00:00:00Z', updated_at: '2026-09-08T00:00:00Z', version: 2,
    }
    const { wrapper, operations } = harness(undefined, [base, { ...base, id: 2, status: 'submitted' }, { ...base, id: 3, status: 'uncertain' }])
    await flushPromises()
    expect(wrapper.findAll('[data-testid="notification-retry"]')).toHaveLength(1)
    await wrapper.get('[data-testid="notification-retry"]').trigger('click')
    await flushPromises()
    expect(operations.filter((op) => op.kind === 'command')).toEqual([{
      kind: 'command', name: 'notification.retry', request: {
        mutation: { optimistic_version: 2, idempotency_key: expect.any(String) }, delivery_id: 1,
      },
    }])
    expect(wrapper.findAll('[data-testid="notification-retry"]')).toHaveLength(0)
  })

})
