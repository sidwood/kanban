// The Execution Profile catalogue state, driven entirely through
// the generated client: the named entries with their closed schema
// — harness, model, effort, usage pool, fallback policy — and the
// define, update, and retire commands that change them (KAN-S7-US1,
// DR-EP-01). Refusals are reported, never swallowed, and the list
// keeps retired entries so history stays visible.
import { defineStore } from 'pinia'
import { KanbanClient } from '@kanban/contracts'
import type { ProfileRecord } from '@kanban/contracts'
import { asApiError } from '../core/transport'
import type { ShellTransport } from '../core/transport'

function mutationFor(optimisticVersion: number) {
  return { optimistic_version: optimisticVersion, idempotency_key: crypto.randomUUID() }
}

export const useProfilesStore = defineStore('profiles', {
  state: () => ({
    profiles: [] as ProfileRecord[],
    loaded: false,
    error: null as string | null,
  }),
  actions: {
    // Load every catalogue entry, retired ones included.
    async refresh(transport: ShellTransport): Promise<void> {
      try {
        const response = await new KanbanClient(transport).queryProfileList({})
        this.profiles = response.profiles
        this.loaded = true
        this.error = null
      } catch (failure) {
        this.error = asApiError(failure).message
      }
    },
    // Define one new named entry. Reports whether it landed; a
    // refusal is reported and the list stands.
    async define(
      transport: ShellTransport,
      draft: {
        name: string
        harness: string
        model: string
        effort: string
        usage_pool: string
        fallback: string
      },
    ): Promise<boolean> {
      try {
        await new KanbanClient(transport).commandProfileDefine({
          mutation: mutationFor(0),
          name: draft.name,
          harness: draft.harness,
          model: draft.model,
          effort: draft.effort,
          usage_pool: draft.usage_pool,
          ...(draft.fallback.trim().length > 0 ? { fallback: draft.fallback.trim() } : {}),
        })
        this.error = null
      } catch (failure) {
        this.error = asApiError(failure).message
        return false
      }
      await this.refresh(transport)
      return true
    },
    // Replace one entry's definition under its own name.
    async update(transport: ShellTransport, profile: ProfileRecord): Promise<boolean> {
      // A cleared or blank fallback is omitted, exactly as define()
      // does, so clearing never sends a blank profile name.
      const fallback = profile.fallback?.trim() ?? ''
      try {
        await new KanbanClient(transport).commandProfileUpdate({
          mutation: mutationFor(profile.version),
          name: profile.name,
          harness: profile.harness,
          model: profile.model,
          effort: profile.effort,
          usage_pool: profile.usage_pool,
          ...(fallback.length > 0 ? { fallback } : {}),
        })
        this.error = null
      } catch (failure) {
        this.error = asApiError(failure).message
        return false
      }
      await this.refresh(transport)
      return true
    },
    // Retire one entry. Retirement is terminal and the entry stays
    // listed.
    async retire(transport: ShellTransport, profile: ProfileRecord): Promise<boolean> {
      try {
        await new KanbanClient(transport).commandProfileRetire({
          mutation: mutationFor(profile.version),
          name: profile.name,
        })
        this.error = null
      } catch (failure) {
        this.error = asApiError(failure).message
        return false
      }
      await this.refresh(transport)
      return true
    },
  },
})
