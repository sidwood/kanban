// The embedded timeline surface's query state and filters. Every read
// is bound to the scope it was issued in, so the activity of a Project
// the operator has left can never answer into the one they are in
// (KAN-T140-AC6, KAN-T145).
import { defineStore } from 'pinia'
import { KanbanClient } from '@kanban/contracts'
import type {
  TimelineEntityKind,
  TimelineEntityRef,
  TimelineEventKind,
  TimelineScope,
} from '@kanban/contracts'
import { asApiError } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import {
  adoptScope,
  emptyScope,
  issueRead,
  releaseScope,
  scopeHolds,
} from '../core/scope-authority'
import type { ScopeClaim } from '../core/scope-authority'
import { datetimeLocalToUtcIso } from './timeline-datetime'

export interface TimelineEventView {
  id: number
  scope: TimelineScope
  kind: TimelineEventKind
  entity?: TimelineEntityRef | null
  recorded_at: string
  detail: unknown
}

export interface TimelineFilters {
  entityKind: TimelineEntityKind | null
  entityId: string
  kinds: TimelineEventKind[]
  since: string
  until: string
}

/** The key one timeline scope is held under. */
function keyOf(scope: TimelineScope): string {
  return scope === 'global' ? 'timeline:global' : `timeline:project:${scope.project}`
}

export const useTimelineStore = defineStore('timeline', {
  state: () => ({
    ...emptyScope(),
    scope: null as TimelineScope | null,
    filters: {
      entityKind: null,
      entityId: '',
      kinds: [] as TimelineEventKind[],
      since: '',
      until: '',
    } as TimelineFilters,
    // The kind filter the operator owns, held while a traced role
    // imposes its own and given back when the trace ends.
    ownedKinds: null as TimelineEventKind[] | null,
    events: [] as TimelineEventView[],
    loading: false,
    error: null as string | null,
  }),
  actions: {
    // Read one scope's activity, taking authority for it first.
    async load(transport: ShellTransport, scope: TimelineScope): Promise<void> {
      const claim = adoptScope(this, keyOf(scope))
      this.scope = scope
      await this.read(transport, scope, claim)
    },
    // Re-read the scope already on display, for example when the
    // operator applies their own filters.
    async refresh(transport: ShellTransport): Promise<void> {
      const scope = this.scope
      if (scope === null) {
        return
      }
      await this.read(transport, scope, issueRead(this))
    },
    // One read, bound to the scope that issued it: an answer the scope
    // no longer holds writes nothing.
    async read(
      transport: ShellTransport,
      scope: TimelineScope,
      claim: ScopeClaim,
    ): Promise<void> {
      this.loading = true
      this.error = null
      try {
        const response = await new KanbanClient(transport).queryTimelineQuery({
          scope,
          entity: this.entityFilter(),
          kinds: this.filters.kinds.length > 0 ? this.filters.kinds : undefined,
          since: datetimeLocalToUtcIso(this.filters.since),
          until: datetimeLocalToUtcIso(this.filters.until, 'end'),
        })
        if (!scopeHolds(this, claim)) return
        this.events = response.events.map((event) => ({
          id: event.id,
          scope: event.scope,
          kind: event.kind,
          entity: event.entity,
          recorded_at: event.recorded_at,
          detail: event.detail,
        }))
      } catch (failure) {
        if (!scopeHolds(this, claim)) return
        this.error = asApiError(failure).message
        this.events = []
      } finally {
        if (scopeHolds(this, claim)) {
          this.loading = false
        }
      }
    },
    // Forget the activity when no scope is on display; anything still
    // in flight is superseded.
    clear(): void {
      releaseScope(this)
      this.scope = null
      this.events = []
      this.loading = false
      this.error = null
    },
    setEntityFilter(kind: TimelineEntityKind | null, id: string): void {
      this.filters.entityKind = kind
      this.filters.entityId = id
    },
    // The operator's own kind filter. Choosing kinds while a role is
    // traced makes the choice theirs, so the trace hands nothing back
    // when it ends.
    setKindFilter(kinds: TimelineEventKind[]): void {
      this.filters.kinds = kinds
      this.ownedKinds = null
    },
    // The kind filter a traced role imposes: the telemetry Herdr
    // reported is the only record this application holds of a role
    // (DR-HB-03). Tracing nothing gives the operator's own filter back
    // rather than clearing what they chose themselves.
    traceRole(role: string | null): void {
      if (role !== null) {
        if (this.ownedKinds === null) {
          this.ownedKinds = [...this.filters.kinds]
        }
        this.filters.kinds = ['telemetry']
        return
      }
      if (this.ownedKinds !== null) {
        this.filters.kinds = this.ownedKinds
        this.ownedKinds = null
      }
    },
    setSince(value: string): void {
      this.filters.since = value
    },
    setUntil(value: string): void {
      this.filters.until = value
    },
    entityFilter(): TimelineEntityRef | undefined {
      if (!this.filters.entityKind || !this.filters.entityId) {
        return undefined
      }
      return {
        kind: this.filters.entityKind,
        id: this.filters.entityId,
      }
    },
  },
})
