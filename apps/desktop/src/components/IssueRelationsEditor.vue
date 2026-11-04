<script setup lang="ts">
import { inject, ref } from 'vue'
import { KanbanClient } from '@kanban/contracts'
import type { IssueRecord } from '@kanban/contracts'
import { asApiError, kanbanTransportKey } from '../core/transport'
import { useIssueOverlayStore } from '../stores/issue-overlay'
import AppButton from './AppButton.vue'
import InlineAlert from './InlineAlert.vue'

const props = defineProps<{
  issue: IssueRecord
}>()

const emit = defineEmits<{
  refreshed: [issue: IssueRecord]
}>()

const transport = inject(kanbanTransportKey)
const overlay = useIssueOverlayStore()
const error = ref<string | null>(null)
const blocksTarget = ref('')
const blockedByTarget = ref('')
const relatedTarget = ref('')
const duplicateTarget = ref('')

async function saveRelations(patch: Record<string, unknown>): Promise<void> {
  if (!transport) return
  error.value = null
  try {
    await new KanbanClient(transport).commandIssueSave({
      mutation: {
        optimistic_version: props.issue.version,
        idempotency_key: crypto.randomUUID(),
      },
      identifier: props.issue.identifier,
      ...patch,
    })
    const number = Number(props.issue.identifier.split('-').pop())
    const code = props.issue.identifier.replace(/-\d+$/, '')
    const refreshed = await overlay.refreshOne(
      transport,
      props.issue.project_id,
      code,
      props.issue.ticket_id,
      number,
    )
    if (refreshed) emit('refreshed', refreshed)
  } catch (failure) {
    error.value = asApiError(failure).message
  }
}

function addBlocks(): void {
  if (!blocksTarget.value.trim()) return
  void saveRelations({ add_blocks: [blocksTarget.value.trim()] })
  blocksTarget.value = ''
}

function addBlockedBy(): void {
  if (!blockedByTarget.value.trim()) return
  void saveRelations({ add_blocked_by: [blockedByTarget.value.trim()] })
  blockedByTarget.value = ''
}

function addRelated(): void {
  if (!relatedTarget.value.trim()) return
  void saveRelations({ add_related: [relatedTarget.value.trim()] })
  relatedTarget.value = ''
}

function setDuplicate(): void {
  if (!duplicateTarget.value.trim()) return
  void saveRelations({ duplicate_of: duplicateTarget.value.trim() })
  duplicateTarget.value = ''
}

function removeRelation(identifier: string, kind: string): void {
  if (kind === 'blocks') void saveRelations({ remove_blocks: [identifier] })
  else if (kind === 'blocked-by') void saveRelations({ remove_blocked_by: [identifier] })
  else if (kind === 'related') void saveRelations({ remove_related: [identifier] })
}
</script>

<template>
  <section
    class="flex flex-col gap-2"
    data-testid="issue-relations"
  >
    <h3 class="font-display text-sm font-semibold tracking-tight text-ink">
      Relations
    </h3>
    <InlineAlert v-if="error">
      {{ error }}
    </InlineAlert>
    <ul
      v-if="issue.relations.length"
      class="flex flex-col gap-1 text-sm"
      data-testid="issue-relation-list"
    >
      <li
        v-for="relation in issue.relations"
        :key="`${relation.kind}-${relation.identifier}`"
        class="flex items-center justify-between gap-2 rounded-control border border-line px-2 py-1"
      >
        <span><span class="font-mono text-xs">{{ relation.kind }}</span> · {{ relation.identifier }}</span>
        <AppButton
          variant="secondary"
          @click="removeRelation(relation.identifier, relation.kind)"
        >
          Remove
        </AppButton>
      </li>
    </ul>
    <div class="grid gap-2 sm:grid-cols-2">
      <label class="flex flex-col gap-1 text-xs text-ink-muted">
        Blocks (issue key)
        <input
          v-model="blocksTarget"
          data-testid="issue-relation-blocks-input"
          class="rounded-control border border-line bg-surface px-2 py-1 text-sm"
          @keydown.stop
        >
      </label>
      <AppButton
        variant="secondary"
        data-testid="issue-relation-blocks-add"
        @click="addBlocks()"
      >
        Add blocks
      </AppButton>
      <label class="flex flex-col gap-1 text-xs text-ink-muted">
        Blocked by
        <input
          v-model="blockedByTarget"
          data-testid="issue-relation-blocked-by-input"
          class="rounded-control border border-line bg-surface px-2 py-1 text-sm"
          @keydown.stop
        >
      </label>
      <AppButton
        variant="secondary"
        @click="addBlockedBy()"
      >
        Add blocked by
      </AppButton>
      <label class="flex flex-col gap-1 text-xs text-ink-muted">
        Related
        <input
          v-model="relatedTarget"
          data-testid="issue-relation-related-input"
          class="rounded-control border border-line bg-surface px-2 py-1 text-sm"
          @keydown.stop
        >
      </label>
      <AppButton
        variant="secondary"
        @click="addRelated()"
      >
        Add related
      </AppButton>
      <label class="flex flex-col gap-1 text-xs text-ink-muted">
        Duplicate of
        <input
          v-model="duplicateTarget"
          data-testid="issue-relation-duplicate-input"
          class="rounded-control border border-line bg-surface px-2 py-1 text-sm"
          @keydown.stop
        >
      </label>
      <AppButton
        variant="secondary"
        @click="setDuplicate()"
      >
        Set duplicate of
      </AppButton>
    </div>
  </section>
</template>
