<script setup lang="ts">
// One card face: the fixed regions — number with Project code, kind,
// title, chips, priority — over a real Ticket the projection
// returned. Presentation only; the chips arrive resolved.
import StatusBadge from '../StatusBadge.vue'
import type { StatusTone } from '../StatusBadge.vue'
import type { CardChip } from '../../views/board-chips'
import { chipSurfaceClass } from '../../views/board-chips'

const {
  ticketId,
  number,
  title,
  kindLabel,
  projectCode,
  showProject,
  chips,
  statusLabel,
  statusTone,
  showsStatus,
  draggable,
  dragging,
  chrome,
  label,
  kind,
  state,
} = defineProps<{
  ticketId: number
  number: string
  title: string
  kindLabel: string
  projectCode: string
  /** Every Project's board names the Project on the face. */
  showProject: boolean
  chips: readonly CardChip[]
  statusLabel: string
  statusTone: StatusTone
  showsStatus: boolean
  draggable: boolean
  dragging: boolean
  chrome: string
  /** The accessible name: the title, and the state where the column cannot say it. */
  label: string
  kind: string
  state: string
}>()

const emit = defineEmits<{
  open: []
  dragstart: [event: DragEvent]
  dragend: []
}>()
</script>

<template>
  <article
    class="flex w-full flex-col gap-2 rounded-control border bg-surface px-3 py-3 text-left shadow-panel transition-[border-color,box-shadow,opacity]"
    :class="[chrome, dragging ? 'opacity-60' : '', draggable ? 'cursor-grab' : '']"
    :draggable="draggable"
    :aria-label="label"
    :data-testid="`kanban-card-${ticketId}`"
    :data-kind="kind"
    :data-state="state"
    @dragstart="emit('dragstart', $event)"
    @dragend="emit('dragend')"
  >
    <div class="flex items-baseline justify-between gap-2">
      <span
        class="font-mono text-[0.6875rem] text-ink-subtle tabular-nums"
        :data-testid="`card-number-${ticketId}`"
      >
        {{ number }}
      </span>
      <span
        class="text-[0.625rem] font-semibold tracking-[0.06em] text-ink-subtle uppercase"
        :data-testid="`card-kind-${ticketId}`"
      >
        {{ kindLabel }}
      </span>
    </div>
    <button
      type="button"
      class="self-start text-left text-sm font-medium text-ink underline-offset-2 transition-colors hover:text-accent hover:underline focus-visible:rounded-control focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent/40"
      :data-testid="`open-ticket-${ticketId}`"
      @click="emit('open')"
    >
      {{ title }}
    </button>
    <ul
      class="flex flex-wrap gap-1"
      :data-testid="`card-chips-${ticketId}`"
      :aria-label="`Chips for ${number}`"
    >
      <li
        v-if="showProject"
        class="inline-flex max-w-full items-center gap-1 rounded-full border border-line bg-surface px-2 py-0.5 text-[0.625rem] leading-[1.3] text-ink-muted"
        :data-testid="`card-project-${ticketId}`"
      >
        <span class="font-semibold tracking-[0.04em] uppercase">Project</span>
        <span class="font-mono">{{ projectCode }}</span>
      </li>
      <li
        v-for="chip in chips"
        :key="chip.kind"
        class="inline-flex max-w-full items-center gap-1 rounded-full border px-2 py-0.5 text-[0.625rem] leading-[1.3]"
        :class="chipSurfaceClass(chip.tone)"
        :data-tone="chip.tone ?? 'neutral'"
        :data-testid="`card-chip-${chip.kind}-${ticketId}`"
        :title="chip.detail"
      >
        <span class="font-semibold tracking-[0.04em] uppercase">
          {{ chip.label }}
        </span>
        <span class="truncate">{{ chip.value }}</span>
        <!-- The fallback indicator an effective profile wears (DR-BP-13). -->
        <span
          v-if="chip.fallback"
          aria-label="fallback profile"
          :data-testid="`card-fallback-${ticketId}`"
        >
          ↺
        </span>
      </li>
    </ul>
    <StatusBadge
      v-if="showsStatus"
      :tone="statusTone"
      :data-testid="`card-status-${ticketId}`"
    >
      {{ statusLabel }}
    </StatusBadge>
  </article>
</template>
