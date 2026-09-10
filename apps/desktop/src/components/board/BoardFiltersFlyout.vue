<script setup lang="ts">
// The Filters flyout: the eight axes, each a select over the values
// the core listed or the closed vocabulary the contracts carry. A
// choice edits the working view; nothing here touches a Ticket.
import AppButton from '../AppButton.vue'
import BoardFlyout from './BoardFlyout.vue'
import type { FilterAxisModel } from '../../views/board-filters'

const { open, axes, activeCount, shown, total, scopedProjectLabel } = defineProps<{
  open: boolean
  axes: readonly FilterAxisModel[]
  activeCount: number
  shown: number
  total: number | null
  /** The Project the scope pins, when one does; its axis is then
   * shown but not offered. */
  scopedProjectLabel: string | null
}>()

const emit = defineEmits<{
  close: []
  change: [axis: FilterAxisModel['axis'], value: string | null]
  clear: []
}>()

function onChange(axis: FilterAxisModel['axis'], event: Event): void {
  const select = event.target
  if (!(select instanceof HTMLSelectElement)) return
  emit('change', axis, select.value === '' ? null : select.value)
}

function selectValue(axis: FilterAxisModel): string {
  if (axis.selected.length > 1) return '__many__'
  return axis.selected[0] ?? ''
}
</script>

<template>
  <BoardFlyout
    :open="open"
    title="Filters"
    :summary="activeCount > 0 ? `${activeCount} active` : 'None active'"
    testid="filters-flyout"
    @close="emit('close')"
  >
    <label
      v-for="axis in axes"
      :key="axis.axis"
      class="flex flex-col gap-1.5"
    >
      <span class="text-[0.6rem] font-bold tracking-[0.11em] text-ink-subtle uppercase">
        {{ axis.label }}
      </span>
      <select
        v-if="axis.axis === 'projects' && scopedProjectLabel !== null"
        :data-testid="`filter-${axis.axis}`"
        :aria-label="`Filter by ${axis.label}`"
        class="h-8 rounded-control border border-line bg-rail px-2 text-xs text-ink-muted"
        disabled
      >
        <option value="">
          {{ scopedProjectLabel }}
        </option>
      </select>
      <select
        v-else
        :data-testid="`filter-${axis.axis}`"
        :aria-label="`Filter by ${axis.label}`"
        :value="selectValue(axis)"
        class="h-8 rounded-control border px-2 text-xs text-ink"
        :class="
          axis.selected.length > 0
            ? 'border-accent-fill bg-accent/8'
            : 'border-line bg-surface'
        "
        @change="onChange(axis.axis, $event)"
      >
        <option value="">
          Any
        </option>
        <option
          v-if="axis.selected.length > 1"
          value="__many__"
          disabled
        >
          {{ axis.selected.length }} selected
        </option>
        <option
          v-for="option in axis.options"
          :key="option.value"
          :value="option.value"
        >
          {{ option.label }}
        </option>
      </select>
    </label>

    <template #footer>
      <AppButton
        variant="secondary"
        size="sm"
        data-testid="filters-clear"
        :disabled="activeCount === 0"
        @click="emit('clear')"
      >
        Clear all
      </AppButton>
      <span
        class="ml-auto text-[0.7rem] text-ink-subtle"
        data-testid="filters-count"
      >
        {{ shown }} of {{ total ?? shown }} tickets
      </span>
      <AppButton
        variant="primary"
        size="sm"
        data-testid="filters-done"
        @click="emit('close')"
      >
        Done
      </AppButton>
    </template>
  </BoardFlyout>
</template>
