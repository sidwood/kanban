import { mount, flushPromises } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import { describe, expect, it, vi } from 'vitest'
import type { ProfileListResponse, ProfileRecord } from '@kanban/contracts'
import router from '../router'
import { kanbanTransportKey } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import ProfilesView from '../views/ProfilesView.vue'
import { useProfilesStore } from './profiles'

function profile(overrides: Partial<ProfileRecord> = {}): ProfileRecord {
  return {
    name: 'standard',
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

function listResponse(profiles: ProfileRecord[]): ProfileListResponse {
  return { profiles }
}

async function mounted(profiles: ProfileRecord[] = [profile()]) {
  // The catalogue the query answers with, mutated by the commands so
  // a refresh reflects what landed.
  const current: ProfileRecord[] = [...profiles]
  const command = vi.fn((name: string, request: unknown) => {
    const asked = request as ProfileRecord & { name: string }
    if (name === 'profile.define') {
      const defined = profile({
        name: asked.name,
        harness: asked.harness,
        model: asked.model,
        effort: asked.effort,
        usage_pool: asked.usage_pool,
        fallback: (asked as { fallback?: string }).fallback ?? null,
      })
      current.push(defined)
      return Promise.resolve(defined)
    }
    if (name === 'profile.update') {
      const index = current.findIndex((entry) => entry.name === asked.name)
      const updated = profile({ ...asked, version: asked.version + 1 })
      if (index >= 0) {
        current[index] = updated
      }
      return Promise.resolve(updated)
    }
    const index = current.findIndex((entry) => entry.name === asked.name)
    const retired = profile({ ...asked, retired: true, version: asked.version + 1 })
    if (index >= 0) {
      current[index] = retired
    }
    return Promise.resolve(retired)
  })
  const query = vi.fn((name: string) => {
    if (name === 'profile.list') {
      return Promise.resolve(listResponse([...current]))
    }
    return Promise.reject(new Error(`unexpected query ${name}`))
  })
  const transport = {
    query,
    command,
    subscribe: () => () => undefined,
    onConnectionChange: () => () => undefined,
  } as unknown as ShellTransport
  router.push('/settings/profiles')
  await router.isReady()
  const wrapper = mount(ProfilesView, {
    global: {
      plugins: [createPinia(), router],
      provide: { [kanbanTransportKey as symbol]: transport },
    },
  })
  await flushPromises()
  return { wrapper, transport, query, command, store: useProfilesStore() }
}

describe('profiles store and view', () => {
  it('loads the catalogue through the generated client and renders the closed schema', async () => {
    const { wrapper, query } = await mounted([
      profile(),
      profile({ name: 'nightly', model: 'haiku', effort: 'medium', fallback: 'standard' }),
      profile({ name: 'spare', retired: true, version: 2 }),
    ])

    expect(query).toHaveBeenCalledWith('profile.list', {})
    const names = wrapper.findAll('[data-testid="profile-name"]').map((node) => node.text())
    expect(names).toEqual(['standard', 'nightly', 'spare'])
    const fallbackInput = wrapper.find('[data-testid="row-fallback-nightly"]').element as HTMLInputElement
    expect(fallbackInput.value).toBe('standard')
    expect(wrapper.find('[data-testid="profile-retired"]').text()).toBe('retired')
  })

  it('defines a profile through the define command and refreshes the list', async () => {
    const { wrapper, command, store } = await mounted([])

    await wrapper.find('[data-testid="define-name"]').setValue('standard')
    await wrapper.find('[data-testid="define-harness"]').setValue('claude-code')
    await wrapper.find('[data-testid="define-model"]').setValue('opus')
    await wrapper.find('[data-testid="define-effort"]').setValue('high')
    await wrapper.find('[data-testid="define-usage-pool"]').setValue('operator')
    await wrapper.find('[data-testid="define-submit"]').trigger('click')
    await flushPromises()

    expect(command).toHaveBeenCalledWith(
      'profile.define',
      expect.objectContaining({
        name: 'standard',
        harness: 'claude-code',
        model: 'opus',
        effort: 'high',
        usage_pool: 'operator',
      }),
    )
    expect(
      (command.mock.calls[0]?.[1] as Record<string, unknown>).fallback,
    ).toBeUndefined()
    expect(store.profiles.map((entry) => entry.name)).toEqual(['standard'])
    expect(wrapper.find('[data-testid="define-name"]').attributes('value')).toBeUndefined()
  })

  it('sends a named fallback only when one is written', async () => {
    const { wrapper, command } = await mounted([profile()])

    await wrapper.find('[data-testid="define-name"]').setValue('nightly')
    await wrapper.find('[data-testid="define-harness"]').setValue('claude-code')
    await wrapper.find('[data-testid="define-model"]').setValue('haiku')
    await wrapper.find('[data-testid="define-effort"]').setValue('medium')
    await wrapper.find('[data-testid="define-usage-pool"]').setValue('operator')
    await wrapper.find('[data-testid="define-fallback"]').setValue(' standard ')
    await wrapper.find('[data-testid="define-submit"]').trigger('click')
    await flushPromises()

    expect(command).toHaveBeenCalledWith(
      'profile.define',
      expect.objectContaining({ name: 'nightly', fallback: 'standard' }),
    )
  })

  it('updates a definition under its own name with the entry version', async () => {
    const { wrapper, command } = await mounted()

    await wrapper.find('[data-testid="row-model-standard"]').setValue('sonnet')
    await wrapper.find('[data-testid="row-update-standard"]').trigger('click')
    await flushPromises()

    expect(command).toHaveBeenCalledWith(
      'profile.update',
      expect.objectContaining({
        name: 'standard',
        model: 'sonnet',
        mutation: expect.objectContaining({ optimistic_version: 1 }),
      }),
    )
  })

  it('clears an existing fallback through update by omitting the field', async () => {
    const { wrapper, command } = await mounted([
      profile(),
      profile({ name: 'nightly', model: 'haiku', effort: 'medium', fallback: 'standard' }),
    ])

    await wrapper.find('[data-testid="row-fallback-nightly"]').setValue('')
    await wrapper.find('[data-testid="row-update-nightly"]').trigger('click')
    await flushPromises()

    expect(command).toHaveBeenCalledWith(
      'profile.update',
      expect.objectContaining({ name: 'nightly' }),
    )
    expect(
      (command.mock.calls[0]?.[1] as Record<string, unknown>).fallback,
    ).toBeUndefined()

    await wrapper.find('[data-testid="row-fallback-nightly"]').setValue('   ')
    await wrapper.find('[data-testid="row-update-nightly"]').trigger('click')
    await flushPromises()

    expect(
      (command.mock.calls[1]?.[1] as Record<string, unknown>).fallback,
    ).toBeUndefined()
  })

  it('sends a rewritten fallback through update, trimmed like define', async () => {
    const { wrapper, command } = await mounted([
      profile(),
      profile({ name: 'nightly', model: 'haiku', effort: 'medium', fallback: 'standard' }),
    ])

    await wrapper.find('[data-testid="row-fallback-nightly"]').setValue(' spare ')
    await wrapper.find('[data-testid="row-update-nightly"]').trigger('click')
    await flushPromises()

    expect(command).toHaveBeenCalledWith(
      'profile.update',
      expect.objectContaining({ name: 'nightly', fallback: 'spare' }),
    )
  })

  it('retires an entry and keeps it listed as retired', async () => {
    const { wrapper, command, store } = await mounted()

    await wrapper.find('[data-testid="row-retire-standard"]').trigger('click')
    await flushPromises()

    expect(command).toHaveBeenCalledWith(
      'profile.retire',
      expect.objectContaining({
        name: 'standard',
        mutation: expect.objectContaining({ optimistic_version: 1 }),
      }),
    )
    expect(store.profiles[0]?.retired).toBe(true)
  })

  it('reports a refused command instead of swallowing it', async () => {
    setActivePinia(createPinia())
    const refusing = {
      query: async () => {
        throw new Error('unexpected query')
      },
      command: async () => {
        throw Object.assign(new Error('the profile name `standard` is already defined'), {
          code: 'invalid_request',
        })
      },
      subscribe: () => () => undefined,
      onConnectionChange: () => () => undefined,
    } as unknown as ShellTransport
    const store = useProfilesStore()

    const failure = await store.define(refusing, {
      name: 'standard',
      harness: 'claude-code',
      model: 'opus',
      effort: 'high',
      usage_pool: 'operator',
      fallback: '',
    })

    expect(failure).toBe(false)
    expect(store.error).toBe('the profile name `standard` is already defined')
  })
})
