<script setup lang="ts">
import { computed, inject, ref, watch } from 'vue'
import { KanbanClient } from '@kanban/contracts'
import type { CommentListItem, IssueRecord } from '@kanban/contracts'
import { asApiError, kanbanTransportKey } from '../core/transport'
import { useIssueOverlayStore } from '../stores/issue-overlay'
import { useTicketDialogStore } from '../stores/ticket-dialog'
import AppButton from './AppButton.vue'
import InlineAlert from './InlineAlert.vue'
import IssueRelationsEditor from './IssueRelationsEditor.vue'

const props = defineProps<{
  issue: IssueRecord
  projectId: number
  projectCode: string
}>()

const emit = defineEmits<{
  refreshed: [issue: IssueRecord]
}>()

const transport = inject(kanbanTransportKey)
const overlay = useIssueOverlayStore()
const dialog = useTicketDialogStore()

const comments = ref<CommentListItem[]>([])
const commentError = ref<string | null>(null)
const commentBody = ref('')
const descriptionLocal = ref('')
const descriptionSaving = ref(false)
const descriptionError = ref<string | null>(null)
const editingDescription = ref(false)

watch(
  () => props.issue,
  (issue) => {
    descriptionLocal.value = issue.description
    editingDescription.value = false
  },
  { immediate: true },
)

const milestoneLabel = computed(() => {
  if (props.issue.milestone_id == null) return '—'
  return (
    overlay.milestoneName(props.projectId, props.issue.milestone_id) ??
    `#${props.issue.milestone_id}`
  )
})

async function loadComments(): Promise<void> {
  if (!transport) return
  commentError.value = null
  try {
    const response = await new KanbanClient(transport).queryCommentList({
      target: { kind: 'ticket', id: String(props.issue.ticket_id) },
      limit: 50,
    })
    comments.value = response.comments
  } catch {
    comments.value = []
  }
}

watch(
  () => props.issue.ticket_id,
  () => {
    void loadComments()
  },
  { immediate: true },
)

async function postComment(): Promise<void> {
  if (!transport || commentBody.value.trim() === '') return
  commentError.value = null
  try {
    await new KanbanClient(transport).commandCommentCreate({
      mutation: { optimistic_version: 0, idempotency_key: crypto.randomUUID() },
      project_id: props.projectId,
      target: { kind: 'ticket', id: String(props.issue.ticket_id) },
      text: commentBody.value,
      author: 'operator',
    })
    commentBody.value = ''
    await loadComments()
  } catch (failure) {
    commentError.value = asApiError(failure).message
  }
}
async function saveDescription(): Promise<void> {
  if (!transport) return
  descriptionSaving.value = true
  descriptionError.value = null
  try {
    await new KanbanClient(transport).commandIssueSave({
      mutation: {
        optimistic_version: props.issue.version,
        idempotency_key: crypto.randomUUID(),
      },
      identifier: props.issue.identifier,
      description: descriptionLocal.value,
    })
    const refreshed = await overlay.refreshOne(
      transport,
      props.projectId,
      props.projectCode,
      props.issue.ticket_id,
      Number(props.issue.identifier.split('-').pop()),
    )
    if (refreshed) emit('refreshed', refreshed)
    editingDescription.value = false
  } catch (failure) {
    descriptionError.value = asApiError(failure).message
  } finally {
    descriptionSaving.value = false
  }
}

function editIssue(): void {
  dialog.openEdit({ projectId: props.projectId, ticketId: props.issue.ticket_id })
}
</script>

<template>
  <div
    class="flex flex-col gap-6"
    data-testid="issue-detail-panel"
  >
    <dl class="grid gap-2 text-sm">
      <div class="flex justify-between gap-4 border-b border-line pb-2">
        <dt class="text-ink-muted">
          Priority
        </dt>
        <dd data-testid="issue-detail-priority">
          {{ issue.priority }}
        </dd>
      </div>
      <div class="flex justify-between gap-4 border-b border-line pb-2">
        <dt class="text-ink-muted">
          Due date
        </dt>
        <dd data-testid="issue-detail-due">
          {{ issue.due_date ?? '—' }}
        </dd>
      </div>
      <div class="flex justify-between gap-4 border-b border-line pb-2">
        <dt class="text-ink-muted">
          Milestone
        </dt>
        <dd data-testid="issue-detail-milestone">
          {{ milestoneLabel }}
        </dd>
      </div>
      <div class="flex justify-between gap-4 border-b border-line pb-2">
        <dt class="text-ink-muted">
          Assignee
        </dt>
        <dd data-testid="issue-detail-assignee">
          —
        </dd>
      </div>
      <div class="flex flex-col gap-1 border-b border-line pb-2">
        <dt class="text-ink-muted">
          Labels
        </dt>
        <dd data-testid="issue-detail-labels">
          {{ issue.labels.length ? issue.labels.join(', ') : '—' }}
        </dd>
      </div>
    </dl>

    <section
      class="flex flex-col gap-2"
      data-testid="issue-detail-description"
    >
      <div class="flex items-center justify-between gap-2">
        <h3 class="font-display text-sm font-semibold tracking-tight text-ink">
          Description
        </h3>
        <AppButton
          v-if="!editingDescription"
          variant="secondary"
          data-testid="issue-description-edit"
          @click="editingDescription = true"
        >
          Edit
        </AppButton>
      </div>
      <InlineAlert v-if="descriptionError">
        {{ descriptionError }}
      </InlineAlert>
      <textarea
        v-if="editingDescription"
        v-model="descriptionLocal"
        data-testid="issue-description-input"
        class="min-h-[8rem] rounded-control border border-line bg-surface px-3 py-2 text-sm text-ink"
        @keydown.stop
      />
      <div
        v-else
        class="rounded-control border border-line bg-surface/70 px-3 py-2 text-sm whitespace-pre-wrap text-ink"
        data-testid="issue-description-body"
      >
        {{ issue.description || 'No description yet.' }}
      </div>
      <AppButton
        v-if="editingDescription"
        variant="primary"
        data-testid="issue-description-save"
        :disabled="descriptionSaving"
        @click="saveDescription()"
      >
        Save description
      </AppButton>
    </section>

    <IssueRelationsEditor
      :issue="issue"
      @refreshed="(record) => emit('refreshed', record)"
    />

    <section
      class="flex flex-col gap-2"
      data-testid="issue-state-history"
    >
      <h3 class="font-display text-sm font-semibold tracking-tight text-ink">
        State history
      </h3>
      <table class="w-full text-left text-sm">
        <thead class="text-xs text-ink-muted uppercase">
          <tr>
            <th class="pb-1 pr-2">
              State
            </th>
            <th class="pb-1 pr-2">
              Started
            </th>
            <th class="pb-1">
              Ended
            </th>
          </tr>
        </thead>
        <tbody>
          <tr
            v-for="(entry, index) in issue.state_history"
            :key="`${entry.state}-${index}`"
            :data-testid="`issue-history-row-${index}`"
          >
            <td class="py-1 pr-2 font-medium text-ink">
              {{ entry.state }}
            </td>
            <td class="py-1 pr-2 font-mono text-xs text-ink-muted">
              {{ entry.started_at }}
            </td>
            <td class="py-1 font-mono text-xs text-ink-muted">
              {{ entry.ended_at ?? '—' }}
            </td>
          </tr>
        </tbody>
      </table>
    </section>

    <section
      class="flex flex-col gap-2"
      data-testid="issue-comments"
    >
      <h3 class="font-display text-sm font-semibold tracking-tight text-ink">
        Comments
      </h3>
      <InlineAlert v-if="commentError">
        {{ commentError }}
      </InlineAlert>
      <ul
        v-if="comments.length"
        class="flex flex-col gap-2"
        data-testid="issue-comment-list"
      >
        <li
          v-for="comment in comments"
          :key="comment.id"
          class="rounded-control border border-line bg-surface/70 px-3 py-2 text-sm"
          :data-testid="`issue-comment-${comment.id}`"
        >
          <p class="mt-1 whitespace-pre-wrap text-ink">
            {{ comment.body }}
          </p>
        </li>
      </ul>
      <p
        v-else
        class="text-sm text-ink-muted"
        data-testid="issue-comment-empty"
      >
        No comments yet.
      </p>
      <label class="flex flex-col gap-1 text-sm text-ink-muted">
        Add comment
        <textarea
          v-model="commentBody"
          data-testid="issue-comment-input"
          rows="3"
          class="rounded-control border border-line bg-surface px-3 py-2 text-sm text-ink"
          @keydown.stop
        />
      </label>
      <AppButton
        variant="primary"
        data-testid="issue-comment-post"
        @click="postComment()"
      >
        Post comment
      </AppButton>
    </section>

    <AppButton
      variant="secondary"
      data-testid="issue-detail-edit"
      @click="editIssue()"
    >
      Edit issue fields
    </AppButton>
  </div>
</template>
