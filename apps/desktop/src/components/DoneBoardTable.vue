<script setup lang="ts">
// The Done table the board demotes its Done column into: the same
// rows, below the board, with the chevron that brings them back.
import AppButton from './AppButton.vue'
import ChevronIcon from './ChevronIcon.vue'
import type { BoardRegisterRow } from '../views/board-card'

defineProps<{
  rows: readonly BoardRegisterRow[]
  dropActive: boolean
}>()

const emit = defineEmits<{
  select: [row: BoardRegisterRow]
  promote: []
  dragover: [event: DragEvent]
  dragleave: []
  drop: [event: DragEvent]
}>()
</script>

<template>
  <section
    class="overflow-hidden rounded-panel border bg-surface transition-colors"
    :class="dropActive ? 'border-accent/50 bg-accent/6' : 'border-line'"
    data-testid="done-table"
    aria-label="Done"
    @dragover="emit('dragover', $event)"
    @dragleave="emit('dragleave')"
    @drop="emit('drop', $event)"
  >
    <header class="flex flex-col gap-0.5 border-b border-line px-3.5 py-3">
      <div class="flex items-center gap-2">
        <h2 class="font-display text-base font-semibold tracking-tight text-ink">
          Done
        </h2>
        <span class="flex-1" />
        <span
          class="rounded-full bg-tint px-2 py-0.5 font-mono text-xs text-ink-muted"
          data-testid="done-count"
        >
          {{ rows.length }}
        </span>
        <AppButton
          variant="secondary"
          size="sm"
          aria-label="Return Done to its column"
          data-testid="bring-done-back-to-board"
          @click="emit('promote')"
        >
          <ChevronIcon direction="up" />
          Return to column
        </AppButton>
      </div>
      <p class="text-[0.6rem] font-semibold tracking-[0.1em] text-ink-subtle uppercase">
        Landed · Complete · Closed
      </p>
    </header>
    <table class="w-full border-collapse text-left">
      <tbody v-if="rows.length > 0">
        <tr
          v-for="row in rows"
          :key="row.ticket.id"
          class="border-t border-line first:border-t-0"
          :data-testid="`done-row-${row.ticket.id}`"
        >
          <td class="w-24 px-3 py-2 font-mono text-[0.6875rem] text-accent tabular-nums">
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
          <td class="w-32 px-3 py-2 text-xs text-ink-muted">
            {{ row.kindLabel }}
          </td>
          <td class="w-16 px-3 py-2 font-mono text-[0.6875rem] text-ink-subtle">
            {{ row.projectCode }}
          </td>
          <td class="w-24 px-3 py-2 text-xs text-ink-muted">
            {{ row.spec ?? '—' }}
          </td>
          <td class="w-28 px-3 py-2 text-right font-mono text-[0.6875rem] text-ink-subtle">
            {{ row.progress }}
          </td>
        </tr>
      </tbody>
      <tbody v-else>
        <tr>
          <td
            colspan="6"
            class="px-3 py-6 text-center text-xs text-ink-subtle"
          >
            Nothing here yet.
          </td>
        </tr>
      </tbody>
    </table>
  </section>
</template>
