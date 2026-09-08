import { flushPromises, mount } from '@vue/test-utils'
import { describe, expect, it } from 'vitest'
import type { TicketRecord, SchedulePreviewResponse, ScheduleRecord } from '@kanban/contracts'
import { kanbanTransportKey } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import ScheduleEditor from './ScheduleEditor.vue'

const ticket = {
  id: 3, project_id: 4, number: 19, kind: 'task', priority: 'normal',
  state: 'draft', title: 'Archive expired logs', slice: null, spec_id: null,
  criteria: [], bug: null, subtype: 'operational', mode: 'agent',
  completion: ['Only expired logs are archived.'], scheduled_for: null, due: null,
  profile: 'standard', version: 7,
} satisfies TicketRecord

const preview = {
  activations: [
    { utc: '2026-03-29T01:00:00.000Z', local: '2026-03-29T02:00:00.000+01:00' },
    { utc: '2026-03-30T00:30:00.000Z', local: '2026-03-30T01:30:00.000+01:00' },
    { utc: '2026-03-31T00:30:00.000Z', local: '2026-03-31T01:30:00.000+01:00' },
    { utc: '2026-04-01T00:30:00.000Z', local: '2026-04-01T01:30:00.000+01:00' },
    { utc: '2026-04-02T00:30:00.000Z', local: '2026-04-02T01:30:00.000+01:00' },
  ],
  dst_behaviour: {
    kind: 'fixed_time',
    spring_forward: 'A fixed clock time in a DST gap runs at the first valid time after the jump.',
    fall_back: 'A fixed clock time in a repeated hour runs once, at its first occurrence.',
  },
} satisfies SchedulePreviewResponse

function harness(
  answerPreview: () => Promise<SchedulePreviewResponse> = () => Promise.resolve(preview),
  stored: ScheduleRecord | null = null,
) {
  const operations: Array<{ kind: string; name: string; request: unknown }> = []
  const transport = {
    query: (name: string, request: unknown) => {
      operations.push({ kind: 'query', name, request })
      if (name === 'schedule.preview') return answerPreview()
      if (name === 'schedule.get') return Promise.resolve({ ticket_id: (request as { ticket_id: number }).ticket_id, schedule: stored })
      return Promise.reject(new Error(`Unexpected query: ${name}`))
    },
    command: (name: string, request: unknown) => {
      operations.push({ kind: 'command', name, request })
      return Promise.resolve({ ...ticket, state: 'scheduled', version: 8 })
    },
    subscribe: () => () => undefined,
    onConnectionChange: () => () => undefined,
  } as unknown as ShellTransport
  const wrapper = mount(ScheduleEditor, {
    props: { tickets: [ticket], projectCode: 'CORE' },
    global: { provide: { [kanbanTransportKey as symbol]: transport } },
  })
  return { wrapper, operations }
}

async function fillRecurring(wrapper: ReturnType<typeof harness>['wrapper']) {
    await wrapper.get('[data-testid="schedule-ticket"]').setValue('3')
    await flushPromises()
    await wrapper.get('[data-testid="schedule-mode"]').setValue('recurring')
    await wrapper.get('[data-testid="schedule-cron"]').setValue('30 1 * * *')
    await wrapper.get('[data-testid="schedule-timezone"]').setValue('Europe/London')
    await wrapper.get('[data-testid="schedule-after"]').setValue('2026-03-28T23:00:00Z')
}

describe('schedule-editor', () => {
  it('shows server-calculated activations and DST rules before saving', async () => {
    const { wrapper, operations } = harness()
    expect(wrapper.find('[data-testid="schedule-ticket"]').exists()).toBe(true)
    await fillRecurring(wrapper)
    expect(wrapper.get('[data-testid="schedule-save"]').attributes('disabled')).toBeDefined()
    await wrapper.get('[data-testid="schedule-preview"]').trigger('click')
    await flushPromises()
    expect(operations).toContainEqual({ kind: 'query', name: 'schedule.preview', request: {
      cron: '30 1 * * *', timezone: 'Europe/London', after: '2026-03-28T23:00:00Z', count: 5,
    } })
    expect(wrapper.text()).toContain('2026-03-29T01:00:00.000Z')
    expect(wrapper.text()).toContain('2026-03-29T02:00:00.000+01:00')
    expect(wrapper.text()).toContain('first valid time')
    expect(wrapper.text()).toContain('runs once')
    expect(operations.filter((op) => op.kind === 'command')).toEqual([])
  })

  it('saves the previewed schedule with the current Ticket version', async () => {
    const { wrapper, operations } = harness()
    await fillRecurring(wrapper)
    await wrapper.get('[data-testid="schedule-preview"]').trigger('click')
    await flushPromises()
    expect(wrapper.get('[data-testid="schedule-save"]').attributes('disabled')).toBeUndefined()
    await wrapper.get('[data-testid="schedule-form"]').trigger('submit')
    await flushPromises()
    expect(operations.filter((op) => op.kind === 'command')).toEqual([{
      kind: 'command', name: 'ticket.schedule', request: {
        mutation: { optimistic_version: 7, idempotency_key: expect.any(String) },
        ticket_id: 3, cron: '30 1 * * *', timezone: 'Europe/London',
        after: '2026-03-28T23:00:00Z', profile: 'standard',
      },
    }])
    expect(wrapper.emitted('saved')).toEqual([[{ ...ticket, state: 'scheduled', version: 8 }]])
    expect(wrapper.text()).toContain('Schedule saved')
  })


  it('invalidates the preview when the operator changes its inputs', async () => {
    const { wrapper, operations } = harness()
    await fillRecurring(wrapper)
    await wrapper.get('[data-testid="schedule-preview"]').trigger('click')
    await flushPromises()
    await wrapper.get('[data-testid="schedule-cron"]').setValue('0 9 * * *')
    expect(wrapper.find('[data-testid="schedule-preview-results"]').exists()).toBe(false)
    expect(wrapper.get('[data-testid="schedule-save"]').attributes('disabled')).toBeDefined()
    await wrapper.get('[data-testid="schedule-form"]').trigger('submit')
    await flushPromises()
    expect(operations.filter((op) => op.kind === 'command')).toEqual([])
  })


  it('does not restore a late preview after the draft changes', async () => {
    let resolvePreview!: (value: SchedulePreviewResponse) => void
    const pending = new Promise<SchedulePreviewResponse>((resolve) => { resolvePreview = resolve })
    const { wrapper, operations } = harness(() => pending)
    await fillRecurring(wrapper)
    await wrapper.get('[data-testid="schedule-preview"]').trigger('click')
    await wrapper.get('[data-testid="schedule-timezone"]').setValue('UTC')
    resolvePreview(preview)
    await flushPromises()
    expect(wrapper.find('[data-testid="schedule-preview-results"]').exists()).toBe(false)
    expect(wrapper.get('[data-testid="schedule-save"]').attributes('disabled')).toBeDefined()
    await wrapper.get('[data-testid="schedule-form"]').trigger('submit')
    expect(operations.filter((op) => op.kind === 'command')).toEqual([])
  })


  it('requires a new preview when the Ticket version changes', async () => {
    const { wrapper, operations } = harness()
    await fillRecurring(wrapper)
    await wrapper.get('[data-testid="schedule-preview"]').trigger('click')
    await flushPromises()
    await wrapper.setProps({ tickets: [{ ...ticket, version: 8 }] })
    expect(wrapper.find('[data-testid="schedule-preview-results"]').exists()).toBe(false)
    await wrapper.get('[data-testid="schedule-form"]').trigger('submit')
    expect(operations.filter((op) => op.kind === 'command')).toEqual([])
  })


  it('loads the standing schedule instead of replacing it with defaults', async () => {
    const { wrapper, operations } = harness(undefined, {
      id: 2, activation: null, cron: '0 7 * * *', timezone: 'UTC',
      profile: 'night-watch', next_activation: '2026-10-26T07:00:00.000Z',
    })
    await wrapper.get('[data-testid="schedule-ticket"]').setValue('3')
    await flushPromises()
    expect(operations).toContainEqual({ kind: 'query', name: 'schedule.get', request: { ticket_id: 3 } })
    expect((wrapper.get('[data-testid="schedule-mode"]').element as HTMLSelectElement).value).toBe('recurring')
    expect((wrapper.get('[data-testid="schedule-cron"]').element as HTMLInputElement).value).toBe('0 7 * * *')
    expect((wrapper.get('[data-testid="schedule-profile"]').element as HTMLInputElement).value).toBe('night-watch')
    expect(wrapper.get('[data-testid="schedule-save"]').attributes('disabled')).toBeDefined()
  })


  it('saves one-time activations without irrelevant cron fields', async () => {
    const oneTime: SchedulePreviewResponse = {
      activations: [{ utc: '2026-10-25T01:30:00.000Z', local: '2026-10-25T01:30:00.000+00:00' }],
      dst_behaviour: {
        kind: 'fixed_instant',
        spring_forward: 'A one-time activation is a fixed instant; DST does not move it.',
        fall_back: 'An explicit UTC offset selects one instant, even in a repeated hour.',
      },
    }
    const { wrapper, operations } = harness(() => Promise.resolve(oneTime))
    await wrapper.setProps({ tickets: [{
      ...ticket, kind: 'implementation', title: null, slice: 'Guard a narrow feature',
      spec_id: 7, criteria: [{ outcome: 'The guard holds.', stories: ['CORE-S1-US1'] }],
      subtype: null, mode: null, completion: [],
    }] })
    await wrapper.get('[data-testid="schedule-ticket"]').setValue('3')
    await flushPromises()
    expect(wrapper.find('[data-testid="schedule-cron"]').exists()).toBe(false)
    expect(wrapper.find('[data-testid="schedule-after"]').exists()).toBe(false)
    await wrapper.get('[data-testid="schedule-activation"]').setValue('2026-10-25T01:30:00+00:00')
    await wrapper.get('[data-testid="schedule-timezone"]').setValue('Europe/London')
    await wrapper.get('[data-testid="schedule-preview"]').trigger('click')
    await flushPromises()
    await wrapper.get('[data-testid="schedule-form"]').trigger('submit')
    await flushPromises()
    expect(operations.filter((op) => op.kind === 'command')).toEqual([{
      kind: 'command', name: 'ticket.schedule', request: {
        mutation: { optimistic_version: 7, idempotency_key: expect.any(String) },
        ticket_id: 3, activation: '2026-10-25T01:30:00+00:00', timezone: 'Europe/London', profile: 'standard',
      },
    }])
  })

})
