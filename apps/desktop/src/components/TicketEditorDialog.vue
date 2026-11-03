<script setup lang="ts">
// The one Ticket editor: a kind-adaptive dialog the operator reaches
// from New Ticket, from a Spec's uncovered Story, and from the
// drawer's Edit, preset to the kind that entry point means for edits.
// Creating and editing both use the Linear issue model via `issue.save`.
import { computed, inject, ref, watch } from 'vue'
import { RouterLink } from 'vue-router'
import { KanbanClient } from '@kanban/contracts'
import type { TicketRecord } from '@kanban/contracts'
import { asApiError, kanbanTransportKey } from '../core/transport'
import { useProjectRegisterStore } from '../stores/project-register'
import { useTicketDialogStore } from '../stores/ticket-dialog'
import type { TicketEditorRequest } from '../stores/ticket-dialog'
import {
  LINEAR_PRIORITIES,
  blankIssueCreateDraft,
  useTicketEditorStore,
} from '../stores/ticket-editor'
import { issueDisplayKey } from '../views/board-card'
import AppButton from './AppButton.vue'
import AppDialog from './AppDialog.vue'
import InlineAlert from './InlineAlert.vue'

const transport = inject(kanbanTransportKey)
const dialog = useTicketDialogStore()
const projects = useProjectRegisterStore()
const editor = useTicketEditorStore()

const projectId = ref<number | null>(null)
const record = ref<TicketRecord | null>(null)
const issueDraft = ref(blankIssueCreateDraft())
const readError = ref<string | null>(null)
const saved = ref(false)

// Every read carries the opening it was issued under, so a dialog
// the operator has closed or re-pointed never renders an answer
// belonging to the record it left.
let opening = 0

const open = computed(() => dialog.editorOpen)
const mode = computed(() => dialog.editor?.mode ?? 'create')
const error = computed(() => readError.value ?? editor.error)

const activeProjects = computed(() => projects.projects.filter((entry) => !entry.archived))

const editIdentifier = computed(() => {
  const held = record.value
  if (!held) return ''
  const project = projects.projects.find((entry) => entry.id === held.project_id)
  return project ? issueDisplayKey(project.code, held.number) : ''
})

const title = computed(() =>
  mode.value === 'create' ? 'New issue' : `Edit ${editIdentifier.value || 'issue'}`,
)

const projectMissing = computed(
  () => projects.loaded && (activeProjects.value.length === 0 || projectId.value === null),
)

watch(() => dialog.editor, (request) => void adopt(request), { immediate: true })

async function adopt(request: TicketEditorRequest | null): Promise<void> {
  opening += 1
  const attempt = opening
  record.value = null
  readError.value = null
  editor.error = null
  saved.value = false
  if (!request || !transport) return

  issueDraft.value = blankIssueCreateDraft()
  if (request.story !== null) {
    issueDraft.value.description = `Covers story ${request.story}.`
  }
  projectId.value = request.projectId

  try {
    if (!projects.loaded || projects.projects.length === 0) {
      await projects.refresh(transport)
    }
    if (attempt !== opening) return
    if (projectId.value === null) {
      const first =
        projects.projects.find((entry) => !entry.archived) ?? projects.projects[0]
      projectId.value = first?.id ?? null
    }
    if (request.mode === 'edit' && request.ticketId !== null) {
      const held = await new KanbanClient(transport).queryTicketGet({
        ticket_id: request.ticketId,
      })
      if (attempt !== opening) return
      record.value = held
      projectId.value = held.project_id
      const heldProject = projects.projects.find((entry) => entry.id === held.project_id)
      const identifier = heldProject
        ? issueDisplayKey(heldProject.code, held.number)
        : `${held.project_id}-${held.number}`
      const issue = await new KanbanClient(transport).queryIssueGet({ identifier })
      if (attempt !== opening) return
      issueDraft.value = {
        title: issue.title,
        description: issue.description,
        priority: issue.priority,
        dueDate: issue.due_date ?? '',
      }
    }
  } catch (failure) {
    if (attempt !== opening) return
    readError.value = asApiError(failure).message
  }
}

async function switchProject(): Promise<void> {
  readError.value = null
}

async function submitCreate(): Promise<void> {
  if (!transport) return
  if (projectMissing.value || projectId.value === null) {
    readError.value = 'Register a project before creating an issue.'
    return
  }
  readError.value = null
  const landed = await editor.createIssue(transport, projectId.value, issueDraft.value)
  if (landed) dialog.closeEditor()
}

async function submitEdit(): Promise<void> {
  const held = record.value
  if (!transport || !held) return
  const project = projects.projects.find((entry) => entry.id === held.project_id)
  if (!project) {
    readError.value = 'Project is missing for this issue.'
    return
  }
  const identifier = issueDisplayKey(project.code, held.number)
  const saved = await editor.updateIssue(
    transport,
    identifier,
    held.version,
    issueDraft.value,
  )
  if (saved) dialog.closeEditor()
}

const FIELD_CLASS =
  'rounded-control border border-line bg-surface px-3 py-2 text-sm text-ink placeholder:text-ink-subtle'
</script>

<template>
  <AppDialog
    :open="open"
    :title="title"
    size="wide"
    testid="ticket-dialog"
    @close="dialog.closeEditor()"
  >
    <template
      v-if="editIdentifier"
      #subtitle
    >
      {{ editIdentifier }} · version {{ record?.version }}
    </template>

    <InlineAlert
      v-if="error"
      data-testid="ticket-error"
    >
      {{ error }}
    </InlineAlert>

    <InlineAlert
      v-if="mode === 'create' && projectMissing"
      data-testid="ticket-project-missing"
    >
      Register a project before creating an issue.
      <RouterLink
        to="/register"
        class="ml-1 underline"
      >
        Register a project
      </RouterLink>
    </InlineAlert>

    <form
      v-if="mode === 'create'"
      class="flex flex-col gap-3"
      @submit.prevent="submitCreate()"
    >
      <label class="flex flex-col gap-1 text-sm text-ink-muted">
        Project
        <select
          v-model="projectId"
          data-testid="ticket-project"
          aria-label="Project"
          :class="FIELD_CLASS"
          @change="switchProject"
        >
          <option
            v-for="entry in activeProjects"
            :key="entry.id"
            :value="entry.id"
          >
            {{ entry.code }} — {{ entry.name }}
          </option>
        </select>
      </label>

      <label class="flex flex-col gap-1 text-sm text-ink-muted">
        Title
        <input
          v-model="issueDraft.title"
          data-testid="ticket-title"
          aria-label="Issue title"
          required
          placeholder="What needs doing?"
          :class="FIELD_CLASS"
        >
      </label>

      <label class="flex flex-col gap-1 text-sm text-ink-muted">
        Description
        <textarea
          v-model="issueDraft.description"
          data-testid="ticket-description"
          aria-label="Issue description"
          rows="4"
          placeholder="Context, acceptance notes, links…"
          :class="FIELD_CLASS"
        />
      </label>

      <div class="flex flex-wrap gap-3">
        <label class="flex flex-col gap-1 text-sm text-ink-muted">
          Priority
          <select
            v-model="issueDraft.priority"
            data-testid="ticket-priority"
            aria-label="Priority"
            :class="FIELD_CLASS"
          >
            <option
              v-for="priority in LINEAR_PRIORITIES"
              :key="priority.value"
              :value="priority.value"
            >
              {{ priority.label }}
            </option>
          </select>
        </label>
        <label class="flex flex-col gap-1 text-sm text-ink-muted">
          Due date (optional)
          <input
            v-model="issueDraft.dueDate"
            data-testid="ticket-due"
            aria-label="Due date, RFC 3339"
            placeholder="2026-09-30T17:00:00Z"
            class="rounded-control border border-line bg-surface px-3 py-2 font-mono text-sm text-ink"
          >
        </label>
      </div>

      <AppButton
        variant="primary"
        type="submit"
        data-testid="ticket-create"
        :disabled="projectMissing"
      >
        Create issue
      </AppButton>
    </form>

    <template v-else>
      <form
        class="flex flex-col gap-3"
        data-testid="ticket-edit-issue-form"
        @submit.prevent="submitEdit()"
      >
        <p
          v-if="editIdentifier"
          class="font-mono text-sm text-ink-muted"
          data-testid="ticket-edit-key"
        >
          {{ editIdentifier }}
        </p>

        <label class="flex flex-col gap-1 text-sm text-ink-muted">
          Title
          <input
            v-model="issueDraft.title"
            data-testid="ticket-title"
            aria-label="Issue title"
            required
            :class="FIELD_CLASS"
          >
        </label>

        <label class="flex flex-col gap-1 text-sm text-ink-muted">
          Description
          <textarea
            v-model="issueDraft.description"
            data-testid="ticket-description"
            aria-label="Issue description"
            rows="4"
            :class="FIELD_CLASS"
          />
        </label>

        <div class="flex flex-wrap gap-3">
          <label class="flex flex-col gap-1 text-sm text-ink-muted">
            Priority
            <select
              v-model="issueDraft.priority"
              data-testid="ticket-priority"
              aria-label="Priority"
              :class="FIELD_CLASS"
            >
              <option
                v-for="priority in LINEAR_PRIORITIES"
                :key="priority.value"
                :value="priority.value"
              >
                {{ priority.label }}
              </option>
            </select>
          </label>
          <label class="flex flex-col gap-1 text-sm text-ink-muted">
            Due date (optional)
            <input
              v-model="issueDraft.dueDate"
              data-testid="ticket-due"
              aria-label="Due date, RFC 3339"
              placeholder="2026-09-30T17:00:00Z"
              class="rounded-control border border-line bg-surface px-3 py-2 font-mono text-sm text-ink"
            >
          </label>
        </div>

        <AppButton
          variant="primary"
          type="submit"
          data-testid="ticket-save-issue"
        >
          Save issue
        </AppButton>
      </form>
    </template>
  </AppDialog>
</template>
