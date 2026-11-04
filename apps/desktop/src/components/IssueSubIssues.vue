<script setup lang="ts">
import { computed, inject, ref, watch } from 'vue'
import { KanbanClient } from '@kanban/contracts'
import type { IssueRecord } from '@kanban/contracts'
import { asApiError, kanbanTransportKey } from '../core/transport'
import { useTicketDialogStore } from '../stores/ticket-dialog'
import AppButton from './AppButton.vue'
import InlineAlert from './InlineAlert.vue'

const props = defineProps<{
  projectCode: string
  ticketId: number
  projectId: number
}>()

const transport = inject(kanbanTransportKey)
const dialog = useTicketDialogStore()

const issue = ref<IssueRecord | null>(null)
const error = ref<string | null>(null)
const loading = ref(false)

const progress = computed(() => {
  const children = issue.value?.children ?? []
  if (children.length === 0) return null
  const doneStates = new Set(['Done', 'Cancelled', 'Superseded', 'Duplicate'])
  const done = children.filter((child) => doneStates.has(child.state)).length
  return { done, total: children.length }
})

watch(
  () => [props.projectCode, props.ticketId] as const,
  async () => {
    if (!transport) return
    loading.value = true
    error.value = null
    try {
      const client = new KanbanClient(transport)
      const ticket = await client.queryTicketGet({ ticket_id: props.ticketId })
      const key = `${props.projectCode}-${ticket.number}`
      issue.value = await client.queryIssueGet({ identifier: key })
    } catch (failure) {
      issue.value = null
      error.value = asApiError(failure).message
    } finally {
      loading.value = false
    }
  },
  { immediate: true },
)

function addSubIssue(): void {
  dialog.openSubIssue({
    projectId: props.projectId,
    parentTicketId: props.ticketId,
  })
}
</script>

<template>
  <section
    class="flex flex-col gap-2 rounded-panel border border-line bg-surface/60 p-3"
    data-testid="drawer-sub-issues"
  >
    <div class="flex flex-wrap items-center justify-between gap-2">
      <h3 class="text-sm font-semibold text-ink">
        Sub-issues
      </h3>
      <span
        v-if="progress"
        class="font-mono text-xs text-ink-muted"
        data-testid="drawer-sub-issue-progress"
      >
        {{ progress.done }} of {{ progress.total }} done
      </span>
    </div>

    <InlineAlert
      v-if="error"
      data-testid="drawer-sub-issue-error"
    >
      {{ error }}
    </InlineAlert>

    <p
      v-else-if="loading"
      class="text-sm text-ink-muted"
    >
      Loading sub-issues…
    </p>

    <ul
      v-else-if="issue?.children?.length"
      class="flex flex-col gap-1 text-sm"
      data-testid="drawer-sub-issue-list"
    >
      <li
        v-for="child in issue.children"
        :key="child.identifier"
        class="flex items-center justify-between gap-2 rounded-control border border-line px-2 py-1"
      >
        <span class="font-mono text-xs text-ink-muted">{{ child.identifier }}</span>
        <span class="min-w-0 flex-1 truncate text-ink">{{ child.title }}</span>
        <span class="text-xs text-ink-muted">{{ child.state }}</span>
      </li>
    </ul>

    <p
      v-else
      class="text-sm text-ink-muted"
      data-testid="drawer-sub-issue-empty"
    >
      No sub-issues yet.
    </p>

    <AppButton
      variant="secondary"
      data-testid="drawer-add-sub-issue"
      @click="addSubIssue()"
    >
      Add sub-issue
    </AppButton>
  </section>
</template>
