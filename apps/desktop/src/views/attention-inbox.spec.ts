import { flushPromises, mount } from '@vue/test-utils'
import { afterEach, describe, expect, it, vi } from 'vitest'
import type { AttentionItemRecord, AttentionState, AttentionListQuery, AttentionAcknowledgeRequest, AttentionListResponse } from '@kanban/contracts'
import { kanbanTransportKey } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import AttentionInboxView from './AttentionInboxView.vue'
import HomeView from './HomeView.vue'
import router from '../router'
import { createPinia } from 'pinia'
import NotificationSettings from '../components/NotificationSettings.vue'

const kinds: AttentionState[] = ['blocker', 'missing_result', 'human_decision', 'review_request', 'failed_schedule', 'invalid_approval', 'disconnected_session', 'stale_run']
const project = {
  id: 1, code: 'CORE', name: 'Control plane', repository: '/repositories/kanban',
  seed_workspace: '/workspaces/kanban.seed', default_branch: 'main', herdr_session: 'kanban-main',
  herdr_workspace: 'kanban.seed', initiative_id: null, archived: false,
  counters: { plan: 0, spec: 0, ticket: 8 }, version: 1,
}
const mounted: Array<{ unmount(): void }> = []
afterEach(() => { mounted.splice(0).forEach((wrapper) => wrapper.unmount()); vi.useRealTimers() })

function harness(answerList?: (query: AttentionListQuery, current: AttentionItemRecord[]) => Promise<AttentionListResponse>) {
  let items: AttentionItemRecord[] = kinds.map((kind, index) => ({
    id: `attention-${index}`, project_id: 1, kind, subject_kind: 'ticket', subject_id: String(index + 1),
    summary: `${kind} needs operator attention`, detail: { sources: [{ ticket_id: index + 1 }] },
    version: 1, active: true, acknowledged_by: null, acknowledged_at: null,
    first_seen_at: '2026-09-08T00:00:00Z', last_seen_at: '2026-09-08T00:00:00Z',
  }))
  const operations: Array<{ kind: string; name: string; request: unknown }> = []
  const transport = {
    query: (name: string, request: unknown) => {
      operations.push({ kind: 'query', name, request })
      if (name === 'attention.list') {
        const query = request as AttentionListQuery
        const current = items.filter((item) => query.include_acknowledged || item.acknowledged_by === null)
        return answerList ? answerList(query, current) : Promise.resolve({ items: current })
      }
      if (name === 'project.list') return Promise.resolve({ projects: [project] })
      if (name === 'notification.settings.get') return Promise.resolve({ project_id: 1, local_enabled: false, mirror_role: null, version: 0 })
      if (name === 'notification.permission.get') return Promise.resolve({ state: 'unavailable', reason: 'Test platform', request_pending: false })
      if (name === 'notification.deliveries') return Promise.resolve({ deliveries: [] })
      return Promise.reject(new Error(`Unexpected query: ${name}`))
    },
    command: (name: string, request: unknown) => {
      operations.push({ kind: 'command', name, request })
      if (name === 'attention.acknowledge') {
        const ack = request as AttentionAcknowledgeRequest
        items = items.map((item) => item.id === ack.item_id ? {
          ...item, acknowledged_by: ack.who, acknowledged_at: '2026-09-08T01:00:00Z', version: item.version + 1,
        } : item)
        return Promise.resolve(items.find((item) => item.id === ack.item_id))
      }
      return Promise.reject(new Error(`Unexpected command: ${name}`))
    },
    subscribe: () => () => undefined,
    onConnectionChange: () => () => undefined,
  } as unknown as ShellTransport
  const wrapper = mount(AttentionInboxView, { global: {
    stubs: { RouterLink: true }, provide: { [kanbanTransportKey as symbol]: transport },
  } })
  mounted.push(wrapper)
  return { wrapper, operations }
}

describe('attention-inbox', () => {
  it('renders every source class without acknowledging on read', async () => {
    const { wrapper, operations } = harness()
    await flushPromises()
    expect(wrapper.findAll('[data-testid="attention-item"]')).toHaveLength(kinds.length)
    for (const kind of kinds) expect(wrapper.text()).toContain(`${kind} needs operator attention`)
    expect(wrapper.get('[data-testid="attention-ack"]').attributes('disabled')).toBeDefined()
    expect(operations.filter((op) => op.kind === 'command')).toEqual([])
  })

  it('acknowledges only the chosen item after an explicit named action', async () => {
    const { wrapper, operations } = harness()
    await flushPromises()
    await wrapper.get('[data-testid="attention-who"]').setValue('Operator A')
    const button = wrapper.findAll('[data-testid="attention-item"]')[0]!.get('[data-testid="attention-ack"]')
    expect(button.attributes('disabled')).toBeUndefined()
    await button.trigger('click')
    await flushPromises()
    expect(operations.filter((op) => op.kind === 'command')).toEqual([{
      kind: 'command', name: 'attention.acknowledge', request: {
        mutation: { optimistic_version: 1, idempotency_key: expect.any(String) },
        item_id: 'attention-0', who: 'Operator A',
      },
    }])
    expect(wrapper.findAll('[data-testid="attention-item"]')).toHaveLength(kinds.length - 1)
  })


  it('shows acknowledgement history without offering a second acknowledgement', async () => {
    const { wrapper } = harness()
    await flushPromises()
    await wrapper.get('[data-testid="attention-who"]').setValue('Operator A')
    await wrapper.get('[data-testid="attention-ack"]').trigger('click')
    await flushPromises()
    expect(wrapper.find('[data-testid="attention-show-ack"]').exists()).toBe(true)
    await wrapper.get('[data-testid="attention-show-ack"]').setValue(true)
    await flushPromises()
    expect(wrapper.findAll('[data-testid="attention-item"]')).toHaveLength(kinds.length)
    expect(wrapper.text()).toContain('Operator A')
    expect(wrapper.text()).toContain('2026-09-08T01:00:00Z')
    expect(wrapper.findAll('[data-testid="attention-item"]')[0]!.get('[data-testid="attention-ack"]').attributes('disabled')).toBeDefined()
  })


  it('ignores an older refresh after a newer source snapshot arrives', async () => {
    let finishOld!: (value: AttentionListResponse) => void
    let oldItems: AttentionItemRecord[] = []
    let calls = 0
    const pending = new Promise<AttentionListResponse>((resolve) => { finishOld = resolve })
    const { wrapper } = harness((_query, current) => {
      calls += 1
      if (calls === 1) { oldItems = current; return pending }
      return Promise.resolve({ items: [] })
    })
    await flushPromises()
    await wrapper.get('[data-testid="attention-show-ack"]').setValue(true)
    await flushPromises()
    finishOld({ items: oldItems })
    await flushPromises()
    expect(wrapper.findAll('[data-testid="attention-item"]')).toHaveLength(0)
  })


  it('refreshes the live projection and stops polling on unmount', async () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] })
    const { wrapper, operations } = harness()
    await flushPromises()
    const before = operations.filter((op) => op.name === 'attention.list').length
    await vi.advanceTimersByTimeAsync(5000)
    await flushPromises()
    expect(operations.filter((op) => op.name === 'attention.list')).toHaveLength(before + 1)
    wrapper.unmount()
    const stopped = operations.length
    await vi.advanceTimersByTimeAsync(10000)
    expect(operations).toHaveLength(stopped)
    expect(operations.filter((op) => op.kind === 'command')).toEqual([])
  })


  it('opens notification preferences without acknowledging or requesting permission', async () => {
    const { wrapper, operations } = harness()
    await flushPromises()
    expect(wrapper.find('[data-testid="attention-notifications"]').exists()).toBe(true)
    await wrapper.get('[data-testid="attention-notifications"]').trigger('click')
    await flushPromises()
    const preferences = wrapper.findComponent(NotificationSettings)
    expect(preferences.exists()).toBe(true)
    expect(preferences.props('projects')).toEqual([project])
    expect(operations.filter((op) => op.kind === 'command')).toEqual([])
  })


  it('is reachable from the home screen and the application router', () => {
    const home = mount(HomeView, { global: { plugins: [createPinia()], stubs: { RouterLink: true }, provide: { [kanbanTransportKey as symbol]: undefined } } })
    mounted.push(home)
    expect(home.find('[data-testid="attention-link"]').exists()).toBe(true)
    expect(home.get('[data-testid="attention-link"]').attributes('to')).toBe('/attention')
    const route = router.getRoutes().find((record) => record.path === '/attention')
    expect(route?.components?.default).toBe(AttentionInboxView)
  })

})
