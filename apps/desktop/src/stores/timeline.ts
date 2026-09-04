// The embedded timeline surface's query state and filters.
import { defineStore } from 'pinia'
import { KanbanClient } from '@kanban/contracts'
import type {
  TimelineEntityKind,
  TimelineEntityRef,
  TimelineEventKind,
} from '@kanban/contracts'
import type { ShellTransport } from '../core/transport'
import { datetimeLocalToUtcIso } from './timeline-datetime'

export interface TimelineEventView {
  id: number
  project_id: string
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
    projectId: '' as string,
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
    async load(transport: ShellTransport, projectId: string): Promise<void> {
      this.projectId = projectId
      await this.refresh(transport)
    },
    async refresh(transport: ShellTransport): Promise<void> {
      if (!this.projectId) {
        return
      }
      this.loading = true
      this.error = null
      try {
        const client = new KanbanClient(transport)
        const response = await client.queryTimelineQuery({
          project_id: this.projectId,
          entity: this.entityFilter(),
          kinds: this.filters.kinds.length > 0 ? this.filters.kinds : undefined,
          since: datetimeLocalToUtcIso(this.filters.since),
          until: datetimeLocalToUtcIso(this.filters.until, 'end'),
        })
        this.events = response.events.map((event) => ({
          id: event.id,
          project_id: event.project_id,
          kind: event.kind,
          entity: event.entity,
          recorded_at: event.recorded_at,
          detail: event.detail,
        }))
      } catch (failure) {
        this.error = failure instanceof Error ? failure.message : String(failure)
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
