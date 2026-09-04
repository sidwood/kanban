<script setup lang="ts">
// Embedded activity timeline with entity, kind, and time filters.
import { computed, inject, onMounted, watch } from 'vue'
import type { TimelineEntityKind, TimelineEventKind } from '@kanban/contracts'
import { kanbanTransportKey } from '../core/transport'
import { useTimelineStore } from '../stores/timeline'

const props = defineProps<{
  projectId: string
  entityKind?: TimelineEntityKind | null
  entityId?: string
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
  if (transport) {
    void timeline.load(transport, props.projectId)
  }
})

watch(
  () => props.projectId,
  (projectId) => {
    if (transport) {
      void timeline.load(transport, projectId)
    }
  },
)
</script>

<template>
  <section
    class="flex w-full max-w-2xl flex-col gap-4 rounded-lg border border-slate-200 bg-white p-4 shadow-sm"
    data-testid="timeline-surface"
  >
    <header class="flex flex-col gap-1">
      <h2 class="text-lg font-semibold text-slate-900">
        Activity timeline
      </h2>
      <p class="text-sm text-slate-500">
        Project {{ projectId }}
      </p>
    </header>

    <form
      class="grid gap-3 md:grid-cols-2"
      @submit.prevent="applyFilters"
    >
      <label class="flex flex-col gap-1 text-sm">
        <span class="text-slate-600">Entity kind</span>
        <select
          v-model="filterEntityKind"
          data-testid="timeline-filter-entity-kind"
          class="rounded border border-slate-300 px-2 py-1"
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
        <span class="text-slate-600">Entity id</span>
        <input
          v-model="filterEntityId"
          data-testid="timeline-filter-entity-id"
          class="rounded border border-slate-300 px-2 py-1"
          placeholder="kan-t9"
        >
      </label>

      <label class="flex flex-col gap-1 text-sm md:col-span-2">
        <span class="text-slate-600">Event kinds</span>
        <select
          v-model="selectedKinds"
          data-testid="timeline-filter-kinds"
          multiple
          class="min-h-28 rounded border border-slate-300 px-2 py-1"
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
        <span class="text-slate-600">Since</span>
        <input
          v-model="since"
          data-testid="timeline-filter-since"
          type="datetime-local"
          class="rounded border border-slate-300 px-2 py-1"
        >
      </label>

      <label class="flex flex-col gap-1 text-sm">
        <span class="text-slate-600">Until</span>
        <input
          v-model="until"
          data-testid="timeline-filter-until"
          type="datetime-local"
          class="rounded border border-slate-300 px-2 py-1"
        >
      </label>

      <button
        type="submit"
        data-testid="timeline-apply-filters"
        class="rounded bg-slate-900 px-3 py-2 text-sm font-medium text-white md:col-span-2"
      >
        Apply filters
      </button>
    </form>

    <p
      v-if="timeline.loading"
      data-testid="timeline-loading"
      class="text-sm text-slate-500"
    >
      Loading timeline…
    </p>
    <p
      v-else-if="timeline.error"
      data-testid="timeline-error"
      class="text-sm text-red-600"
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
        class="text-sm text-slate-500"
      >
        No events match the current filters.
      </li>
      <li
        v-for="event in timeline.events"
        :key="event.id"
        class="rounded border border-slate-100 bg-slate-50 px-3 py-2 text-sm"
        :data-testid="`timeline-event-${event.id}`"
      >
        <div class="flex items-center justify-between gap-2">
          <span class="font-medium text-slate-900">{{ event.kind }}</span>
          <time class="text-xs text-slate-500">{{ event.recorded_at }}</time>
        </div>
        <p
          v-if="event.entity"
          class="text-xs text-slate-600"
        >
          {{ event.entity.kind }} · {{ event.entity.id }}
        </p>
      </li>
    </ul>
  </section>
</template>
