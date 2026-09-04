// The embedded timeline surface's query state and filters.
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

export const useTimelineStore = defineStore('timeline', {
  state: () => ({
    scope: null as TimelineScope | null,
    filters: {
      entityKind: null,
      entityId: '',
      kinds: [] as TimelineEventKind[],
      since: '',
      until: '',
    } as TimelineFilters,
    events: [] as TimelineEventView[],
    loading: false,
    error: null as string | null,
  }),
  actions: {
    async load(transport: ShellTransport, scope: TimelineScope): Promise<void> {
      this.scope = scope
      await this.refresh(transport)
    },
    async refresh(transport: ShellTransport): Promise<void> {
      if (this.scope === null) {
        return
      }
      this.loading = true
      this.error = null
      try {
        const client = new KanbanClient(transport)
        const response = await client.queryTimelineQuery({
          scope: this.scope,
          entity: this.entityFilter(),
          kinds: this.filters.kinds.length > 0 ? this.filters.kinds : undefined,
          since: datetimeLocalToUtcIso(this.filters.since),
          until: datetimeLocalToUtcIso(this.filters.until, 'end'),
        })
        this.events = response.events.map((event) => ({
          id: event.id,
          scope: event.scope,
          kind: event.kind,
          entity: event.entity,
          recorded_at: event.recorded_at,
          detail: event.detail,
        }))
      } catch (failure) {
        this.error = asApiError(failure).message
        this.events = []
      } finally {
        this.loading = false
      }
    },
    setEntityFilter(kind: TimelineEntityKind | null, id: string): void {
      this.filters.entityKind = kind
      this.filters.entityId = id
    },
    setKindFilter(kinds: TimelineEventKind[]): void {
      this.filters.kinds = kinds
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
