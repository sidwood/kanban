import { flushPromises, mount } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it } from 'vitest'
import type {
  TicketRecord,
  TicketReviewConfigRecord,
  TicketReviewConfigResponse,
} from '@kanban/contracts'
import { kanbanTransportKey } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import ReviewConfigEditor from './ReviewConfigEditor.vue'

const ticket = {
  id: 2,
  project_id: 4,
  number: 18,
  kind: 'bug' as const,
  priority: 'urgent' as const,
  state: 'draft' as const,
  spec_id: null,
  title: 'Landing drops the integration branch',
  slice: null,
  criteria: [],
  bug: null,
  subtype: null,
  mode: null,
  completion: [],
  scheduled_for: null,
  due: null,
  profile: 'implementer',
  version: 1,
} satisfies TicketRecord

function standing(overrides: Partial<TicketReviewConfigRecord> = {}): TicketReviewConfigRecord {
  return {
    ticket_id: 2,
    stages: [
      {
        slots: [
          { occupant: { kind: 'profile', name: 'outsider' }, requirement: 'required' },
          { occupant: { kind: 'human' }, requirement: 'optional' },
        ],
      },
    ],
    version: 1,
    ...overrides,
  }
}

// A transport steered per operation name, recording every command.
function harness(config: TicketReviewConfigRecord | null, failure?: unknown) {
  const commands: Array<[string, unknown]> = []
  let current = config
  const transport = {
    command: (name: string, request: unknown) => {
      commands.push([name, request])
      if (failure) {
        return Promise.reject(failure)
      }
      const asked = request as TicketReviewConfigRecord
      current = standing({ stages: asked.stages, version: (current?.version ?? 0) + 1 })
      return Promise.resolve(current)
    },
    query: (name: string, request: unknown) => {
      commands.push([name, request])
      return Promise.resolve({ config: current } satisfies TicketReviewConfigResponse)
    },
  }
  return { transport: transport as ShellTransport, commands }
}

async function mounted(config: TicketReviewConfigRecord | null, failure?: unknown) {
  const { transport, commands } = harness(config, failure)
  const wrapper = mount(ReviewConfigEditor, {
    global: {
      provide: { [kanbanTransportKey as symbol]: transport },
      plugins: [createPinia()],
    },
    props: { tickets: [ticket], projectCode: 'CORE' },
  })
  await flushPromises()
  return { wrapper, commands }
}

async function pick(wrapper: Awaited<ReturnType<typeof mounted>>['wrapper']) {
  await wrapper.find('[data-testid="review-ticket-pick"]').setValue('2')
  await flushPromises()
}

describe('the staged review editor', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
  })

  it('renders nothing editable until a Ticket is picked', async () => {
    const { wrapper } = await mounted(null)

    expect(wrapper.find('[data-testid="review-stage-0"]').exists()).toBe(false)
    expect(wrapper.find('[data-testid="review-config-standing"]').exists()).toBe(false)
  })

  it('seeds the editor from the standing configuration', async () => {
    const { wrapper } = await mounted(standing())
    await pick(wrapper)

    expect(wrapper.find('[data-testid="review-config-standing"]').text()).toContain(
      'version 1',
    )
    expect(
      (wrapper.find('[data-testid="review-slot-profile-0-0"]').element as HTMLInputElement)
        .value,
    ).toBe('outsider')
    expect(
      (wrapper.find('[data-testid="review-slot-occupant-0-1"]').element as HTMLSelectElement)
        .value,
    ).toBe('human')
    expect(
      (wrapper.find('[data-testid="review-slot-requirement-0-1"]').element as HTMLSelectElement)
        .value,
    ).toBe('optional')
  })

  it('composes a blank configuration and replaces the whole set', async () => {
    const { wrapper, commands } = await mounted(null)
    await pick(wrapper)

    await wrapper.find('[data-testid="review-slot-profile-0-0"]').setValue('outsider')
    await wrapper.find('[data-testid="review-slot-add-0"]').trigger('click')
    await wrapper.find('[data-testid="review-slot-occupant-0-1"]').setValue('human')
    await wrapper.find('[data-testid="review-slot-requirement-0-1"]').setValue('optional')
    await wrapper.find('[data-testid="review-stage-add"]').trigger('click')
    await wrapper.find('[data-testid="review-slot-profile-1-0"]').setValue('same-harness')
    await wrapper.find('[data-testid="review-config-configure"]').trigger('submit')
    await flushPromises()

    const configure = commands.filter(([name]) => name === 'ticket.review.configure')
    expect(configure).toEqual([
      [
        'ticket.review.configure',
        {
          mutation: expect.objectContaining({ optimistic_version: 0 }),
          ticket_id: 2,
          stages: [
            {
              slots: [
                {
                  occupant: { kind: 'profile', name: 'outsider' },
                  requirement: 'required',
                },
                { occupant: { kind: 'human' }, requirement: 'optional' },
              ],
            },
            {
              slots: [
                {
                  occupant: { kind: 'profile', name: 'same-harness' },
                  requirement: 'required',
                },
              ],
            },
          ],
        },
      ],
    ])
    expect(wrapper.find('[data-testid="review-config-standing"]').text()).toContain(
      'version 1',
    )
  })

  it('reports a separation refusal while the standing configuration survives', async () => {
    const refusal = {
      code: 'invalid_request',
      message:
        'the reviewer profile `same-model` shares the implementer’s `opus` model family',
    }
    const { wrapper } = await mounted(standing(), refusal)
    await pick(wrapper)

    await wrapper.find('[data-testid="review-slot-profile-0-0"]').setValue('same-model')
    await wrapper.find('[data-testid="review-config-configure"]').trigger('submit')
    await flushPromises()

    const error = wrapper.find('[data-testid="review-config-error"]')
    expect(error.exists()).toBe(true)
    expect(error.text()).toContain('model family')
    expect(wrapper.find('[data-testid="review-config-standing"]').text()).toContain(
      'version 1',
    )
  })
})
