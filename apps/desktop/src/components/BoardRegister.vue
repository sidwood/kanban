<script setup lang="ts">
import StatusBadge from './StatusBadge.vue'
import type { BoardRegisterColumn, BoardRegisterRow } from '../views/board-card'

defineProps<{
  columns: readonly BoardRegisterColumn[]
  moving: boolean
}>()

const emit = defineEmits<{
  select: [row: BoardRegisterRow]
  move: [row: BoardRegisterRow, column: BoardRegisterRow['moves'][number]['column']]
}>()

/**
 * The select is an action, not a state: the row is filed by which table it
 * sits in, so the control returns to its prompt rather than holding the
 * target it was just asked for.
 */
function onMove(row: BoardRegisterRow, event: Event): void {
  const select = event.target
  if (!(select instanceof HTMLSelectElement)) return
  const move = row.moves.find((entry) => entry.column === select.value)
  select.value = ''
  if (move === undefined) return
  emit('move', row, move.column)
}

function headings(column: BoardRegisterColumn): readonly string[] {
  return column.showsStatus
    ? ['Number', 'Ticket', 'Kind', 'Status', 'Move']
    : ['Number', 'Ticket', 'Kind', 'Move']
}
</script>

<template>
  <div
    class="flex flex-col gap-4"
    data-testid="board-register"
  >
    <div
      v-for="column in columns"
      :key="column.id"
      :data-testid="`register-column-${column.id}`"
    >
      <table class="w-full border-collapse text-left">
        <caption class="flex items-baseline justify-between gap-2 px-2 pb-2 text-left">
          <span class="flex flex-col">
            <span class="font-display text-sm font-semibold tracking-tight text-ink">
              {{ column.label }}
            </span>
            <span class="text-[0.625rem] tracking-wide text-ink-subtle uppercase">
              {{ column.subheading }}
            </span>
          </span>
          <span
            class="rounded-full bg-line px-2 py-0.5 font-mono text-xs text-ink-muted"
            :data-testid="`register-count-${column.id}`"
          >
            {{ column.rows.length }}
          </span>
        </caption>
        <thead>
          <tr class="border-b border-line text-[0.625rem] tracking-wide text-ink-subtle uppercase">
            <th
              v-for="heading in headings(column)"
              :key="heading"
              scope="col"
              class="px-2 py-2 font-semibold"
            >
              {{ heading }}
            </th>
          </tr>
        </thead>
        <tbody v-if="column.rows.length > 0">
          <tr
            v-for="row in column.rows"
            :key="row.ticket.id"
            class="border-b border-line last:border-b-0"
            :data-testid="`register-row-${row.ticket.id}`"
          >
            <td class="px-2 py-3 font-mono text-[0.6875rem] text-ink-subtle tabular-nums">
              {{ row.number }}
            </td>
            <td class="px-2 py-3">
              <button
                type="button"
                class="font-medium text-ink underline-offset-2 transition-colors hover:text-accent hover:underline focus-visible:rounded-control focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent/40"
                :aria-label="row.title"
                :data-testid="`open-ticket-${row.ticket.id}`"
                @click="emit('select', row)"
              >
                {{ row.title }}
              </button>
            </td>
            <td class="px-2 py-3 text-sm text-ink-muted">
              {{ row.kindLabel }}
            </td>
            <td
              v-if="column.showsStatus"
              class="px-2 py-3"
            >
              <StatusBadge
                :tone="row.statusTone"
                :data-testid="`register-status-${row.ticket.id}`"
              >
                {{ row.statusLabel }}
              </StatusBadge>
            </td>
            <td class="px-2 py-3 last:pr-0">
              <template v-if="row.moves.length > 0">
                <label
                  class="sr-only"
                  :for="`move-${row.ticket.id}`"
                >
                  Move {{ row.title }}
                </label>
                <select
                  :id="`move-${row.ticket.id}`"
                  class="rounded-control border border-line-strong bg-surface px-2 py-1 text-xs text-ink disabled:opacity-50"
                  :disabled="moving"
                  :data-testid="`move-${row.ticket.id}`"
                  @change="onMove(row, $event)"
                >
                  <option value="">
                    Move to…
                  </option>
                  <option
                    v-for="move in row.moves"
                    :key="move.column"
                    :value="move.column"
                  >
                    {{ move.label }}
                  </option>
                </select>
              </template>
              <span
                v-else
                class="text-ink-subtle"
              >—</span>
            </td>
          </tr>
        </tbody>
        <tbody v-else>
          <tr>
            <td
              :colspan="headings(column).length"
              class="px-2 py-6 text-center text-xs text-ink-subtle"
            >
              Nothing here yet.
            </td>
          </tr>
        </tbody>
      </table>
    </div>
  </div>
</template>
