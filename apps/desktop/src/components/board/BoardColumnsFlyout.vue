<script setup lang="ts">
// The Columns flyout: per group, hide or show — which the Saved View
// owns — and collapse or expand — a presentation preference kept
// beside the theme. Both change presentation only.
import AppButton from '../AppButton.vue'
import BoardFlyout from './BoardFlyout.vue'
import type { BoardGroupId } from '../../views/board-layout'

export interface ColumnPreference {
  group: BoardGroupId
  label: string
  count: number
  hidden: boolean
  /** True once every column the group currently shows is collapsed. */
  collapsed: boolean
  /** How many of the group's columns stand collapsed, and how many
   * it shows: an expanded group is several columns, and a control
   * that says only "Expanded" over a half-collapsed group lies. */
  collapsedColumns: number
  columns: number
}

const { open, rows, summary } = defineProps<{
  open: boolean
  rows: readonly ColumnPreference[]
  summary: string
}>()

/** What the collapse control says: the whole group's state, or how
 * much of an expanded group is collapsed. */
function collapseLabel(row: ColumnPreference): string {
  if (row.collapsed) return 'Collapsed'
  if (row.collapsedColumns > 0) return `${row.collapsedColumns} of ${row.columns} collapsed`
  return 'Expanded'
}

const emit = defineEmits<{
  close: []
  toggleHidden: [group: BoardGroupId]
  toggleCollapsed: [group: BoardGroupId]
  showAll: []
}>()
</script>

<template>
  <BoardFlyout
    :open="open"
    title="Columns"
    :summary="summary"
    testid="columns-flyout"
    @close="emit('close')"
  >
    <p class="text-[0.72rem] leading-relaxed text-ink-subtle">
      Hiding removes a column from the layout and is saved with the view. Collapsing leaves a
      narrow rail with its name and live count. Both change presentation only.
    </p>
    <div
      v-for="row in rows"
      :key="row.group"
      :data-testid="`column-pref-${row.group}`"
      class="rounded-control border border-line px-3 py-2.5"
      :class="row.hidden ? 'bg-rail opacity-75' : 'bg-surface'"
    >
      <div class="flex items-center gap-2">
        <span class="flex-1 text-xs font-semibold text-ink">{{ row.label }}</span>
        <span class="rounded-full bg-tint px-2 py-px font-mono text-[0.65rem] text-ink-muted">
          {{ row.count }}
        </span>
      </div>
      <div class="mt-2 flex gap-2">
        <button
          type="button"
          :data-testid="`column-pref-hide-${row.group}`"
          :aria-pressed="row.hidden"
          :aria-label="`${row.hidden ? 'Show' : 'Hide'} the ${row.label} column`"
          class="h-7 flex-1 rounded-full border px-2.5 text-[0.7rem] font-medium transition-colors"
          :class="
            row.hidden
              ? 'border-caution/40 bg-caution/10 text-caution'
              : 'border-line bg-surface text-ink-muted hover:border-line-strong'
          "
          @click="emit('toggleHidden', row.group)"
        >
          {{ row.hidden ? 'Hidden' : 'Visible' }}
        </button>
        <button
          type="button"
          :data-testid="`column-pref-collapse-${row.group}`"
          :aria-pressed="row.collapsed"
          :aria-label="`${row.collapsed ? 'Expand' : 'Collapse'} the ${row.label} column`"
          :title="collapseLabel(row)"
          class="h-7 flex-1 rounded-full border px-2.5 text-[0.7rem] font-medium transition-colors"
          :class="
            row.collapsed
              ? 'border-info/40 bg-info/10 text-info'
              : 'border-line bg-surface text-ink-muted hover:border-line-strong'
          "
          @click="emit('toggleCollapsed', row.group)"
        >
          {{ collapseLabel(row) }}
        </button>
      </div>
    </div>

    <template #footer>
      <AppButton
        variant="secondary"
        size="sm"
        data-testid="columns-show-all"
        @click="emit('showAll')"
      >
        Show all
      </AppButton>
      <span class="flex-1" />
      <AppButton
        variant="primary"
        size="sm"
        data-testid="columns-done"
        @click="emit('close')"
      >
        Done
      </AppButton>
    </template>
  </BoardFlyout>
</template>
