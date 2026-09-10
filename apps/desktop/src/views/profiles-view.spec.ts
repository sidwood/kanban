// The Execution Profile surface (KAN-T140-AC4, KAN-T140-AC5): the real
// catalogue with the fallback the policy plans and the fallback the
// Runs actually took, and the ordinary implementer assignment, which
// must be possible before any review is configured (KAN-S7-US1,
// KAN-S7-US3, DR-EP-03).
import { flushPromises, mount } from '@vue/test-utils'
import type { VueWrapper } from '@vue/test-utils'
import { createPinia } from 'pinia'
import { afterEach, describe, expect, it, vi } from 'vitest'
import type {
  ProfileRecord,
  ProjectRecord,
  RunRecord,
  TicketRecord,
} from '@kanban/contracts'
import router from '../router'
import { kanbanTransportKey } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import ProfilesView from './ProfilesView.vue'

const project: ProjectRecord = {
  id: 1,
  code: 'CORE',
  name: 'Control plane',
  repository: '/repositories/kanban',
  seed_workspace: '/workspaces/kanban.seed',
  default_branch: 'main',
  herdr_session: 'kanban-main',
  herdr_workspace: 'kanban.seed',
  initiative_id: null,
  archived: false,
  counters: { plan: 0, spec: 0, ticket: 9 },
  version: 1,
}

function profile(overrides: Partial<ProfileRecord> & { name: string }): ProfileRecord {
  return {
    harness: 'claude-code',
    model: 'opus',
    effort: 'high',
    usage_pool: 'operator',
    fallback: null,
    retired: false,
    version: 1,
    ...overrides,
  }
}

const catalogue = [
  profile({ name: 'deep', fallback: 'standard' }),
  profile({ name: 'standard' }),
  profile({ name: 'legacy', fallback: 'withdrawn' }),
  profile({ name: 'withdrawn', retired: true }),
]

function snapshot(name: string) {
  return { name, harness: 'claude-code', model: 'opus', effort: 'high', usage_pool: 'operator' }
}

const runs: RunRecord[] = [
  {
    id: 1,
    project_id: 1,
    ticket_id: 5,
    dispatch_request_id: 2,
    requested: snapshot('deep'),
    effective: snapshot('standard'),
    fallback: true,
    fallback_path: ['deep', 'standard'],
    status: 'superseded',
    created_at: 1789000000,
    version: 1,
  },
]

const ticket = {
  id: 5,
  project_id: 1,
  number: 12,
  kind: 'implementation',
  priority: 'high',
  state: 'draft',
  spec_id: 2,
  slice: 'Recover the support surfaces.',
  criteria: [],
  bug: null,
  completion: [],
  profile: null,
  pinned_spec_version: null,
  version: 4,
} as unknown as TicketRecord

function harness(options: { refuse?: string; catalogue?: ProfileRecord[] } = {}) {
  const operations: Array<{ kind: 'query' | 'command'; name: string; request: unknown }> = []
  let tickets: TicketRecord[] = [ticket]
  const query = vi.fn((name: string, request: unknown) => {
    operations.push({ kind: 'query', name, request })
    switch (name) {
      case 'profile.list':
        return Promise.resolve({ profiles: options.catalogue ?? catalogue })
      case 'project.list':
        return Promise.resolve({ projects: [project] })
      case 'ticket.list':
        return Promise.resolve({ tickets })
      case 'run.list':
        return Promise.resolve({ project_id: 1, runs })
      default:
        return Promise.reject(new Error(`unexpected query ${name}`))
    }
  })
  const command = vi.fn((name: string, request: unknown) => {
    operations.push({ kind: 'command', name, request })
    if (name !== 'ticket.assign') return Promise.resolve({})
    if (options.refuse) {
      return Promise.reject({ code: 'invalid_request', message: options.refuse })
    }
    const body = request as { ticket_id: number; profile: string }
    tickets = tickets.map((entry) =>
      entry.id === body.ticket_id
        ? { ...entry, profile: body.profile, version: entry.version + 1 }
        : entry,
    )
    return Promise.resolve(tickets.find((entry) => entry.id === body.ticket_id))
  })
  const transport = {
    query,
    command,
    subscribe: () => () => undefined,
    onConnectionChange: () => () => undefined,
  } as unknown as ShellTransport
  return { transport, operations }
}

const mounted: VueWrapper[] = []
afterEach(() => {
  for (const wrapper of mounted.splice(0)) wrapper.unmount()
})

async function mountView(options: { refuse?: string; catalogue?: ProfileRecord[] } = {}) {
  const state = harness(options)
  await router.push('/settings/profiles')
  await router.isReady()
  const wrapper = mount(ProfilesView, {
    global: {
      plugins: [createPinia(), router],
      provide: { [kanbanTransportKey as symbol]: state.transport },
    },
  })
  mounted.push(wrapper)
  await flushPromises()
  return { wrapper, ...state }
}

describe('ProfilesView catalogue', () => {
  it('reads the authoritative catalogue, retired entries included', async () => {
    const { wrapper } = await mountView()

    expect(wrapper.findAll('[data-testid="profile-row"]')).toHaveLength(4)
    expect(wrapper.get('[data-testid="profile-retired"]').text()).toBe('retired')
  })

  it('ends the walk at an entry the catalogue still assigns', async () => {
    const { wrapper } = await mountView()

    const walk = wrapper.get('[data-testid="profile-planned-deep"]').text()
    expect(walk).not.toContain('deep \u2192 standard')
    expect(walk).toContain('Its policy names standard, which the core reads only once deep is retired')
  })

  it('never reads the unused successor of an entry that answers', async () => {
    const { wrapper } = await mountView()

    const walk = wrapper.get('[data-testid="profile-planned-legacy"]')
    expect(walk.text()).not.toContain('legacy \u2192 withdrawn')
    expect(walk.text()).not.toContain('withdrawn is retired')
  })

  it('crosses retired hops to the entry the resolver would run', async () => {
    const restored = [
      profile({ name: 'alpha', fallback: 'beta', retired: true }),
      profile({ name: 'beta', fallback: 'gamma', retired: true }),
      profile({ name: 'gamma' }),
    ]

    const { wrapper } = await mountView({ catalogue: restored })

    const walk = wrapper.get('[data-testid="profile-planned-alpha"]')
    expect(walk.text()).toContain('alpha \u2192 beta \u2192 gamma')
    expect(walk.text()).toContain('A run requesting alpha runs gamma')
    expect(walk.text()).toContain('alpha, beta are retired')
    expect(walk.text()).toContain('nothing assigns to them directly')
  })

  it('says a walk of retired entries resolves to nothing', async () => {
    const exhausted = [
      profile({ name: 'alpha', fallback: 'beta', retired: true }),
      profile({ name: 'beta', retired: true }),
    ]

    const { wrapper } = await mountView({ catalogue: exhausted })

    const walk = wrapper.get('[data-testid="profile-planned-alpha"]')
    expect(walk.text()).toContain('alpha \u2192 beta')
    expect(walk.text()).toContain('is retired and names no fallback')
    expect(walk.text()).not.toContain('the walk crosses')
    expect(walk.text()).toContain('No entry on this walk is assignable')
  })

  it('says where a crossed policy names an entry the catalogue does not hold', async () => {
    const dangling = [profile({ name: 'alpha', fallback: 'ghost', retired: true })]

    const { wrapper } = await mountView({ catalogue: dangling })

    const walk = wrapper.get('[data-testid="profile-planned-alpha"]')
    expect(walk.text()).toContain('alpha \u2192 ghost')
    expect(walk.text()).toContain('names no catalogue entry')
  })

  it('never calls an unused dangling successor a refusal', async () => {
    const dangling = [profile({ name: 'alpha', fallback: 'ghost' })]

    const { wrapper } = await mountView({ catalogue: dangling })

    const walk = wrapper.get('[data-testid="profile-planned-alpha"]')
    expect(walk.text()).not.toContain('names no catalogue entry')
    expect(walk.text()).not.toContain('the core refuses')
  })

  it('says where a crossed policy returns to an entry already walked', async () => {
    const knotted = [
      profile({ name: 'alpha', fallback: 'beta', retired: true }),
      profile({ name: 'beta', fallback: 'alpha', retired: true }),
    ]

    const { wrapper } = await mountView({ catalogue: knotted })

    const walk = wrapper.get('[data-testid="profile-planned-alpha"]')
    expect(walk.text()).toContain('alpha \u2192 beta \u2192 alpha')
    expect(walk.text()).toContain('is already on this walk')
  })

  it('reports the fallback the Runs actually took, separately from the plan', async () => {
    const { wrapper } = await mountView()

    const effective = wrapper.get('[data-testid="profile-effective-deep"]')
    expect(effective.text()).toContain('1 of 1')
    expect(effective.text()).toContain('deep → standard')
    expect(wrapper.get('[data-testid="profile-effective-standard"]').text()).toContain('No Run')
  })
})

describe('ProfilesView assignment', () => {
  it('assigns an implementer through the production command with no review configured', async () => {
    const { wrapper, operations } = await mountView()

    await wrapper.get('[data-testid="assign-ticket"]').setValue('5')
    await wrapper.get('[data-testid="assign-profile"]').setValue('deep')
    await wrapper.get('[data-testid="assign-submit"]').trigger('submit')
    await flushPromises()

    expect(operations.filter((entry) => entry.kind === 'command')).toEqual([
      {
        kind: 'command',
        name: 'ticket.assign',
        request: {
          mutation: { optimistic_version: 4, idempotency_key: expect.any(String) },
          ticket_id: 5,
          profile: 'deep',
        },
      },
    ])
    expect(
      operations.some((entry) => entry.name.startsWith('ticket.review')),
      'assignment must not need a review configuration first',
    ).toBe(false)
    expect(wrapper.get('[data-testid="assign-ticket-5-profile"]').text()).toBe('deep')
  })

  it('offers only entries the catalogue still assigns', async () => {
    const { wrapper } = await mountView()

    const options = wrapper
      .get('[data-testid="assign-profile"]')
      .findAll('option')
      .map((entry) => (entry.element as HTMLOptionElement).value)
      .filter((value) => value.length > 0)

    expect(options).toEqual(['deep', 'standard', 'legacy'])
  })

  it('reports a refused assignment and leaves the Ticket unassigned', async () => {
    const { wrapper } = await mountView({ refuse: 'the profile name `deep` is not in the catalogue' })

    await wrapper.get('[data-testid="assign-ticket"]').setValue('5')
    await wrapper.get('[data-testid="assign-profile"]').setValue('deep')
    await wrapper.get('[data-testid="assign-submit"]').trigger('submit')
    await flushPromises()

    expect(wrapper.get('[data-testid="assign-error"]').text()).toContain('not in the catalogue')
    expect(wrapper.get('[data-testid="assign-ticket-5-profile"]').text()).toBe('unassigned')
  })
})
