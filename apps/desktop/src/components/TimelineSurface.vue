<script setup lang="ts">
// Embedded activity timeline with entity, kind, and time filters.
// An observed role can be traced through it: the surface then reads
// the telemetry rows Herdr reported and marks the ones that name that
// exact role, which is the only record this application holds of one
// (KAN-T140-AC6, DR-HB-03).
import { computed, inject, onMounted, watch } from 'vue'
import type { TimelineEntityKind, TimelineEventKind, TimelineScope } from '@kanban/contracts'
import { kanbanTransportKey } from '../core/transport'
import { useTimelineStore } from '../stores/timeline'

const props = defineProps<{
  scope: TimelineScope
  entityKind?: TimelineEntityKind | null
  entityId?: string
  /** The observed role a link is tracing, when one is named. */
  role?: string | null
}>()

const transport = inject(kanbanTransportKey)
const timeline = useTimelineStore()

const eventKinds: TimelineEventKind[] = [
  'transition',
  'run',
  'telemetry',
  'review',
  'finding',
  'evidence',
  'comment',
  'deferral',
  'ruling',
]

const entityKinds: TimelineEntityKind[] = [
  'initiative',
  'project',
  'plan',
  'spec',
  'ticket',
  'run',
  'review',
  'finding',
  'evidence',
  'comment',
]

// The scope is a value, not an identity: reload when what it names
// changes, not when the caller hands over an equal object.
const scopeKey = computed(() =>
  props.scope === 'global' ? 'global' : `project:${props.scope.project}`,
)

const scopeLabel = computed(() =>
  props.scope === 'global' ? 'Everything above Projects' : `Project ${props.scope.project}`,
)

/** The role one event names, when the row carries one: a Herdr
 * telemetry row records the role it reported for. */
function eventRole(detail: unknown): string | null {
  if (typeof detail !== 'object' || detail === null) return null
  const value = (detail as Record<string, unknown>).role
  return typeof value === 'string' ? value : null
}

function tracesRole(detail: unknown): boolean {
  return props.role != null && eventRole(detail) === props.role
}

const selectedKinds = computed({
  get: () => timeline.filters.kinds,
  set: (value: TimelineEventKind[]) => timeline.setKindFilter(value),
})

const filterEntityKind = computed({
  get: () => timeline.filters.entityKind,
  set: (value: TimelineEntityKind | null) => {
    timeline.setEntityFilter(value, timeline.filters.entityId)
  },
})

const filterEntityId = computed({
  get: () => timeline.filters.entityId,
  set: (value: string) => {
    timeline.setEntityFilter(timeline.filters.entityKind, value)
  },
})

const since = computed({
  get: () => timeline.filters.since,
  set: (value: string) => timeline.setSince(value),
})

const until = computed({
  get: () => timeline.filters.until,
  set: (value: string) => timeline.setUntil(value),
})

async function applyFilters(): Promise<void> {
  if (transport) {
    await timeline.refresh(transport)
  }
}

onMounted(() => {
  if (props.entityKind) {
    timeline.setEntityFilter(props.entityKind, props.entityId ?? '')
  }
  adoptRoute()
})

watch([scopeKey, () => props.role], () => {
  adoptRoute()
})

// The route names both the scope and the role, and the surface follows
// both in either direction: naming a role hands the kind filter to it,
// and leaving the role gives the operator's own filter back
// (KAN-T140-AC6).
function adoptRoute(): void {
  timeline.traceRole(props.role ?? null)
  if (transport) {
    void timeline.load(transport, props.scope)
  }
}
</script>

<template>
  <section
    class="flex w-full max-w-2xl min-w-0 flex-col gap-4 rounded-panel border border-line bg-surface p-4"
    data-testid="timeline-surface"
  >
    <header class="flex flex-col gap-1">
      <h2 class="font-display text-lg font-semibold tracking-tight text-ink">
        Activity timeline
      </h2>
      <p class="text-sm text-ink-subtle">
        {{ scopeLabel }}
      </p>
    </header>

    <form
      class="grid gap-3 md:grid-cols-2"
      @submit.prevent="applyFilters"
    >
      <label class="flex flex-col gap-1 text-sm">
        <span class="text-ink-muted">Entity kind</span>
        <select
          v-model="filterEntityKind"
          data-testid="timeline-filter-entity-kind"
          class="min-w-0 rounded-control border border-line bg-surface px-2 py-1 text-ink"
        >
          <option :value="null">
            Any
          </option>
          <option
            v-for="kind in entityKinds"
            :key="kind"
            :value="kind"
          >
            {{ kind }}
          </option>
        </select>
      </label>

      <label class="flex flex-col gap-1 text-sm">
        <span class="text-ink-muted">Entity id</span>
        <input
          v-model="filterEntityId"
          data-testid="timeline-filter-entity-id"
          class="min-w-0 rounded-control border border-line bg-surface px-2 py-1 text-ink"
          placeholder="Entity id"
        >
      </label>

      <label class="flex flex-col gap-1 text-sm md:col-span-2">
        <span class="text-ink-muted">Event kinds</span>
        <select
          v-model="selectedKinds"
          data-testid="timeline-filter-kinds"
          multiple
          class="min-h-28 min-w-0 rounded-control border border-line bg-surface px-2 py-1 text-ink"
        >
          <option
            v-for="kind in eventKinds"
            :key="kind"
            :value="kind"
          >
            {{ kind }}
          </option>
        </select>
      </label>

      <label class="flex flex-col gap-1 text-sm">
        <span class="text-ink-muted">Since</span>
        <input
          v-model="since"
          data-testid="timeline-filter-since"
          type="datetime-local"
          class="min-w-0 rounded-control border border-line bg-surface px-2 py-1 text-ink"
        >
      </label>

      <label class="flex flex-col gap-1 text-sm">
        <span class="text-ink-muted">Until</span>
        <input
          v-model="until"
          data-testid="timeline-filter-until"
          type="datetime-local"
          class="min-w-0 rounded-control border border-line bg-surface px-2 py-1 text-ink"
        >
      </label>

      <button
        type="submit"
        data-testid="timeline-apply-filters"
        class="bg-brand-gradient text-cta-ink shadow-panel rounded-control px-3 py-2 text-sm font-medium transition-[filter] hover:brightness-105 md:col-span-2"
      >
        Apply filters
      </button>
    </form>

    <p
      v-if="timeline.loading"
      data-testid="timeline-loading"
      class="text-sm text-ink-subtle"
    >
      Loading timeline…
    </p>
    <p
      v-else-if="timeline.error"
      data-testid="timeline-error"
      class="text-sm text-critical"
    >
      {{ timeline.error }}
    </p>
    <ul
      v-else
      data-testid="timeline-events"
      class="flex flex-col gap-2"
    >
      <li
        v-if="timeline.events.length === 0"
        class="text-sm text-ink-subtle"
      >
        No events match the current filters.
      </li>
      <li
        v-for="event in timeline.events"
        :key="event.id"
        class="min-w-0 rounded-control border px-3 py-2 text-sm"
        :class="tracesRole(event.detail)
          ? 'border-l-4 border-accent/40 border-l-accent bg-accent/10'
          : 'border-line bg-rail'"
        :data-testid="`timeline-event-${event.id}`"
        :data-linked="tracesRole(event.detail) ? 'true' : undefined"
      >
        <div class="flex flex-wrap items-center justify-between gap-x-2 gap-y-1">
          <span class="font-medium text-ink">{{ event.kind }}</span>
          <span
            v-if="eventRole(event.detail)"
            class="rounded-control bg-surface px-2 py-0.5 font-mono text-xs break-all text-ink-muted"
            :class="tracesRole(event.detail) ? 'ring-1 ring-accent/50' : undefined"
          >role {{ eventRole(event.detail) }}</span>
          <time class="ml-auto text-xs text-ink-subtle">{{ event.recorded_at }}</time>
        </div>
        <p
          v-if="event.entity"
          class="text-xs break-words text-ink-muted"
        >
          {{ event.entity.kind }} · {{ event.entity.id }}
        </p>
      </li>
    </ul>
  </section>
</template>
