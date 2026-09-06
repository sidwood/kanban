<script setup lang="ts">
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
  <div
    class="transition-colors"
    :class="{
      'rounded-panel border border-accent/50 bg-accent/6': dropActive,
    }"
    data-testid="done-table"
    @dragover="emit('dragover', $event)"
    @dragleave="emit('dragleave')"
    @drop="emit('drop', $event)"
  >
    <table class="w-full border-collapse text-left">
      <caption class="flex items-baseline justify-between gap-2 px-2 pb-2 text-left">
        <span class="flex flex-col">
          <span class="font-display text-sm font-semibold tracking-tight text-ink">
            Done
          </span>
          <span class="text-[0.625rem] tracking-wide text-ink-subtle uppercase">
            Landed · Complete · Closed
          </span>
        </span>
        <span class="flex items-center gap-1">
          <span
            class="rounded-full bg-line px-2 py-0.5 font-mono text-xs text-ink-muted"
            data-testid="done-count"
          >
            {{ rows.length }}
          </span>
          <AppButton
            variant="ghost"
            size="iconSm"
            aria-label="Bring Done back to the board"
            data-testid="bring-done-back-to-board"
            @click="emit('promote')"
          >
            <ChevronIcon direction="up" />
          </AppButton>
        </span>
      </caption>
      <thead>
        <tr
          class="border-b border-line text-[0.625rem] tracking-wide text-ink-subtle uppercase"
        >
          <th
            v-for="heading in ['Number', 'Ticket', 'Kind']"
            :key="heading"
            scope="col"
            class="px-2 py-2 font-semibold"
          >
            {{ heading }}
          </th>
        </tr>
      </thead>
      <tbody v-if="rows.length > 0">
        <tr
          v-for="row in rows"
          :key="row.ticket.id"
          class="border-b border-line last:border-b-0"
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
          <td class="px-2 py-3 text-sm text-ink-muted last:pr-0">
            {{ row.kindLabel }}
          </td>
        </tr>
      </tbody>
      <tbody v-else>
        <tr>
          <td
            colspan="3"
            class="px-2 py-6 text-center text-xs text-ink-subtle"
          >
            Nothing here yet.
          </td>
        </tr>
      </tbody>
    </table>
  </div>
</template>
