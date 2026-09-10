<script setup lang="ts">
// The register: one stacked table per visible board column, the same
// live projection the board renders. Task rows move by naming a
// column; the agent-owned kinds say so instead of offering one.
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
    ? ['ID', 'Title', 'Kind', 'Proj', 'Spec', 'State', 'Priority', 'Progress', 'Movement']
    : ['ID', 'Title', 'Kind', 'Proj', 'Spec', 'Priority', 'Progress', 'Movement']
}
</script>

<template>
  <div
    class="flex flex-col gap-3"
    data-testid="board-register"
  >
    <section
      v-for="column in columns"
      :key="column.id"
      :data-testid="`register-column-${column.id}`"
      :aria-label="column.label"
      class="overflow-hidden rounded-panel border border-line bg-surface"
    >
      <header class="flex flex-col gap-0.5 border-b border-line px-3.5 py-3">
        <div class="flex items-center gap-2">
          <h2 class="font-display text-base font-semibold tracking-tight text-ink">
            {{ column.label }}
          </h2>
          <span class="flex-1" />
          <span
            class="rounded-full bg-tint px-2 py-0.5 font-mono text-xs text-ink-muted"
            :data-testid="`register-count-${column.id}`"
          >
            {{ column.rows.length }}
          </span>
        </div>
        <p class="text-[0.6rem] font-semibold tracking-[0.1em] text-ink-subtle uppercase">
          {{ column.subheading }}
        </p>
      </header>
      <div class="overflow-x-auto">
        <table class="w-full min-w-[46rem] border-collapse text-left">
          <thead>
            <tr class="bg-rail text-[0.6rem] tracking-[0.1em] text-ink-subtle uppercase">
              <th
                v-for="heading in headings(column)"
                :key="heading"
                scope="col"
                class="px-3 py-1.5 font-mono font-medium last:text-right"
              >
                {{ heading }}
              </th>
            </tr>
          </thead>
          <tbody v-if="column.rows.length > 0">
            <tr
              v-for="row in column.rows"
              :key="row.ticket.id"
              class="border-t border-line"
              :data-testid="`register-row-${row.ticket.id}`"
            >
              <td class="px-3 py-2 font-mono text-[0.6875rem] text-accent tabular-nums">
                {{ row.number }}
              </td>
              <td class="px-3 py-2">
                <button
                  type="button"
                  class="text-left text-sm font-medium text-ink underline-offset-2 transition-colors hover:text-accent hover:underline focus-visible:rounded-control focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent/40"
                  :aria-label="row.title"
                  :data-testid="`open-ticket-${row.ticket.id}`"
                  @click="emit('select', row)"
                >
                  {{ row.title }}
                </button>
              </td>
              <td class="px-3 py-2 text-xs text-ink-muted">
                {{ row.kindLabel }}
              </td>
              <td class="px-3 py-2 font-mono text-[0.6875rem] text-ink-subtle">
                {{ row.projectCode }}
              </td>
              <td class="px-3 py-2 text-xs text-ink-muted">
                {{ row.spec ?? '—' }}
              </td>
              <td
                v-if="column.showsStatus"
                class="px-3 py-2"
              >
                <StatusBadge
                  :tone="row.statusTone"
                  density="compact"
                  :data-testid="`register-status-${row.ticket.id}`"
                >
                  {{ row.statusLabel }}
                </StatusBadge>
              </td>
              <td class="px-3 py-2">
                <StatusBadge
                  :tone="row.priorityTone"
                  density="compact"
                >
                  {{ row.priorityLabel }}
                </StatusBadge>
              </td>
              <td class="px-3 py-2 font-mono text-[0.6875rem] text-ink-subtle">
                {{ row.progress }}
              </td>
              <td class="px-3 py-2 text-right">
                <template v-if="row.moves.length > 0">
                  <label
                    class="sr-only"
                    :for="`move-${row.ticket.id}`"
                  >
                    Move {{ row.number }} to
                  </label>
                  <select
                    :id="`move-${row.ticket.id}`"
                    class="h-7 rounded-control border border-line-strong bg-surface px-1.5 text-xs text-ink disabled:opacity-50"
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
                  v-else-if="row.agentOwned"
                  class="text-xs text-ink-subtle"
                  :data-testid="`register-agent-owned-${row.ticket.id}`"
                >Agent-owned</span>
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
                class="px-3 py-6 text-center text-xs text-ink-subtle"
              >
                Nothing here yet.
              </td>
            </tr>
          </tbody>
        </table>
      </div>
    </section>
  </div>
</template>
