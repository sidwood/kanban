<script setup lang="ts">
import { computed, inject, ref, watch } from 'vue'
import { KanbanClient } from '@kanban/contracts'
import type { IssueRecord } from '@kanban/contracts'
import { asApiError, kanbanTransportKey } from '../core/transport'
import { useIssueOverlayStore } from '../stores/issue-overlay'
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
const issueOverlay = useIssueOverlayStore()

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

const emit = defineEmits<{
  openIssue: [ticketId: number]
}>()

async function reload(): Promise<void> {
  if (!transport) return
  loading.value = true
  error.value = null
  try {
    const client = new KanbanClient(transport)
    const ticket = await client.queryTicketGet({ ticket_id: props.ticketId })
    const key = `${props.projectCode}-${ticket.number}`
    issue.value = await issueOverlay.readIssue(transport, key)
  } catch (failure) {
    issue.value = null
    error.value = asApiError(failure).message
  } finally {
    loading.value = false
  }
}

watch(
  () => [props.projectCode, props.ticketId] as const,
  async () => {
    await reload()
  },
  { immediate: true },
)

watch(
  () => dialog.editorOpen,
  (open, wasOpen) => {
    if (wasOpen && !open) void reload()
  },
)

defineExpose({ reload })

function openChild(child: { identifier: string }): void {
  if (!transport) return
  void (async () => {
    try {
      const loaded = await issueOverlay.readIssue(transport, child.identifier)
      emit('openIssue', loaded.ticket_id)
    } catch {
      // ignore navigation failures in the list row
    }
  })()
}

function addSubIssue(): void {
  dialog.openSubIssue({
    projectId: props.projectId,
    parentTicketId: props.ticketId,
  })
}

const reordering = ref(false)

async function moveChild(index: number, direction: -1 | 1): Promise<void> {
  if (!transport || !issue.value?.children?.length) return
  const targetIndex = index + direction
  const children = issue.value.children
  if (targetIndex < 0 || targetIndex >= children.length) return

  reordering.value = true
  error.value = null
  const client = new KanbanClient(transport)
  const a = children[index]
  const b = children[targetIndex]
  try {
    const [loadedA, loadedB] = await Promise.all([
      issueOverlay.readIssue(transport, a.identifier),
      issueOverlay.readIssue(transport, b.identifier),
    ])
    await client.commandIssueSave({
      mutation: {
        optimistic_version: loadedA.version,
        idempotency_key: crypto.randomUUID(),
      },
      identifier: a.identifier,
      sort_order: b.sort_order,
    })
    await client.commandIssueSave({
      mutation: {
        optimistic_version: loadedB.version,
        idempotency_key: crypto.randomUUID(),
      },
      identifier: b.identifier,
      sort_order: a.sort_order,
    })
    await reload()
  } catch (failure) {
    error.value = asApiError(failure).message
  } finally {
    reordering.value = false
  }
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
        v-for="(child, index) in issue.children"
        :key="child.identifier"
        class="flex cursor-pointer items-center justify-between gap-2 rounded-control border border-line px-2 py-1 hover:bg-surface/80"
        :data-testid="`drawer-sub-issue-row-${child.identifier}`"
        @click="openChild(child)"
      >
        <button
          type="button"
          class="flex min-w-0 flex-1 cursor-[inherit] items-center gap-2 text-left focus-visible:rounded-control focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent/40"
          :data-testid="`drawer-sub-issue-open-${child.identifier}`"
        >
          <span class="font-mono text-xs text-ink-muted">{{ child.identifier }}</span>
          <span class="min-w-0 flex-1 truncate text-ink">{{ child.title }}</span>
        </button>
        <span class="text-xs text-ink-muted">{{ child.state }}</span>
        <span
          v-if="issue.children.length > 1"
          class="flex shrink-0 gap-1"
          @click.stop
        >
          <AppButton
            variant="secondary"
            :disabled="reordering || index === 0"
            :data-testid="`drawer-sub-issue-up-${child.identifier}`"
            @click="moveChild(index, -1)"
          >
            Up
          </AppButton>
          <AppButton
            variant="secondary"
            :disabled="reordering || index === issue.children.length - 1"
            :data-testid="`drawer-sub-issue-down-${child.identifier}`"
            @click="moveChild(index, 1)"
          >
            Down
          </AppButton>
        </span>
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
