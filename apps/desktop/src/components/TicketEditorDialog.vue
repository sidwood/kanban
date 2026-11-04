<script setup lang="ts">
// The one Ticket editor: a kind-adaptive dialog the operator reaches
// from New Ticket, from a Spec's uncovered Story, and from the
// drawer's Edit, preset to the kind that entry point means for edits.
// Creating and editing both use the Linear issue model via `issue.save`.
import { computed, inject, ref, watch } from 'vue'
import { RouterLink } from 'vue-router'
import { KanbanClient } from '@kanban/contracts'
import type { IssueLabelRecord, IssueMilestoneRecord, TicketRecord } from '@kanban/contracts'
import { asApiError, kanbanTransportKey } from '../core/transport'
import { useProjectRegisterStore } from '../stores/project-register'
import { useTicketDialogStore } from '../stores/ticket-dialog'
import type { TicketEditorRequest } from '../stores/ticket-dialog'
import {
  LINEAR_PRIORITIES,
  blankIssueCreateDraft,
  storyCoverDraft,
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
const heldLabels = ref<string[]>([])
const localTitle = ref('')
const localDescription = ref('')
const availableLabels = ref<IssueLabelRecord[]>([])
const availableMilestones = ref<IssueMilestoneRecord[]>([])
const selectedLabel = ref('')
const readError = ref<string | null>(null)
const parentKey = computed(() => {
  const parentId = dialog.editor?.parentTicketId
  if (parentId == null || projectId.value == null) return null
  const project = projects.projects.find((entry) => entry.id === projectId.value)
  if (!project) return null
  const parent = editor.tickets.find((entry) => entry.id === parentId)
  return parent ? issueDisplayKey(project.code, parent.number) : `#${parentId}`
})

function syncLocalsFromDraft(): void {
  localTitle.value = issueDraft.value.title
  localDescription.value = issueDraft.value.description
}

function flushLocalsToDraft(): void {
  issueDraft.value = {
    ...issueDraft.value,
    title: localTitle.value,
    description: localDescription.value,
  }
}

async function loadPickers(forProject: number): Promise<void> {
  if (!transport) return
  const client = new KanbanClient(transport)
  const [labels, milestones] = await Promise.all([
    client.queryIssueList_labels({ project_id: forProject }),
    client.queryIssueList_milestones({ project_id: forProject }),
  ])
  availableLabels.value = labels.labels
  availableMilestones.value = milestones.milestones
}

function toggleLabel(name: string): void {
  const label = availableLabels.value.find((entry) => entry.name === name)
  const group = label?.group ?? null
  const selected = new Set(issueDraft.value.labels)
  if (selected.has(name)) {
    selected.delete(name)
  } else {
    if (group && (label as IssueLabelRecord & { group_single_select?: boolean }).group_single_select) {
      for (const entry of availableLabels.value) {
        if (entry.group === group) selected.delete(entry.name)
      }
    }
    selected.add(name)
  }
  issueDraft.value = { ...issueDraft.value, labels: [...selected] }
}

function addLabelFromPicker(): void {
  const name = selectedLabel.value.trim()
  if (name === '') return
  toggleLabel(name)
  selectedLabel.value = ''
}

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
  heldLabels.value = []
  readError.value = null
  editor.error = null
  if (!request || !transport) return

  issueDraft.value = blankIssueCreateDraft()
  localTitle.value = ''
  localDescription.value = ''
  if (request.story !== null) {
    issueDraft.value.description = `Covers story ${request.story}.`
    localDescription.value = issueDraft.value.description
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
    if (projectId.value !== null) {
      await editor.refresh(transport, projectId.value)
      await loadPickers(projectId.value)
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
      heldLabels.value = [...issue.labels]
      issueDraft.value = {
        title: issue.title,
        description: issue.description,
        priority: issue.priority,
        dueDate: issue.due_date?.slice(0, 10) ?? '',
        labels: [...issue.labels],
        milestoneId: issue.milestone_id ?? null,
      }
      syncLocalsFromDraft()
    } else {
      syncLocalsFromDraft()
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
  flushLocalsToDraft()
  const request = dialog.editor
  const landed =
    request?.specId != null && request.story !== null
      ? await editor.create(
          transport,
          projectId.value,
          storyCoverDraft(request.specId, request.story, issueDraft.value),
        )
      : await editor.createIssue(
          transport,
          projectId.value,
          issueDraft.value,
          request?.parentTicketId ?? null,
        )
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
  flushLocalsToDraft()
  const savedIssue = await editor.updateIssue(
    transport,
    identifier,
    held.version,
    issueDraft.value,
    heldLabels.value,
  )
  if (savedIssue) dialog.closeEditor()
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
          v-model="localTitle"
          data-testid="ticket-title"
          aria-label="Issue title"
          required
          placeholder="What needs doing?"
          :class="FIELD_CLASS"
          @keydown.stop
        >
      </label>

      <label class="flex flex-col gap-1 text-sm text-ink-muted">
        Description
        <textarea
          v-model="localDescription"
          data-testid="ticket-description"
          aria-label="Issue description"
          rows="4"
          placeholder="Context, acceptance notes, links…"
          :class="FIELD_CLASS"
          @keydown.stop
        />
      </label>

      <p
        v-if="parentKey"
        class="text-sm text-ink-muted"
        data-testid="ticket-parent"
      >
        Parent issue · {{ parentKey }}
      </p>

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
            type="date"
            data-testid="ticket-due"
            aria-label="Due date"
            class="rounded-control border border-line bg-surface px-3 py-2 text-sm text-ink"
          >
        </label>
        <label class="flex flex-col gap-1 text-sm text-ink-muted">
          Milestone
          <select
            v-model="issueDraft.milestoneId"
            data-testid="ticket-milestone"
            aria-label="Milestone"
            :class="FIELD_CLASS"
          >
            <option :value="null">
              None
            </option>
            <option
              v-for="milestone in availableMilestones"
              :key="milestone.id"
              :value="milestone.id"
            >
              {{ milestone.name }}
            </option>
          </select>
        </label>
      </div>

      <fieldset class="flex flex-col gap-2 text-sm text-ink-muted">
        <legend>Labels</legend>
        <label
          v-for="label in availableLabels"
          :key="label.name"
          class="flex items-center gap-2"
        >
          <input
            type="checkbox"
            :checked="issueDraft.labels.includes(label.name)"
            :data-testid="`ticket-label-${label.name}`"
            @change="toggleLabel(label.name)"
          >
          {{ label.name }}
          <span
            v-if="label.group"
            class="text-xs text-ink-subtle"
          >({{ label.group }})</span>
        </label>
        <div class="flex gap-2">
          <input
            v-model="selectedLabel"
            data-testid="ticket-label-add"
            placeholder="Add label by name"
            class="flex-1 rounded-control border border-line bg-surface px-2 py-1 text-sm"
            @keydown.stop
          >
          <AppButton
            variant="secondary"
            type="button"
            data-testid="ticket-label-add-button"
            @click="addLabelFromPicker()"
          >
            Add
          </AppButton>
        </div>
      </fieldset>

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
            v-model="localTitle"
            data-testid="ticket-title"
            aria-label="Issue title"
            required
            :class="FIELD_CLASS"
            @keydown.stop
          >
        </label>

        <label class="flex flex-col gap-1 text-sm text-ink-muted">
          Description
          <textarea
            v-model="localDescription"
            data-testid="ticket-description"
            aria-label="Issue description"
            rows="4"
            :class="FIELD_CLASS"
            @keydown.stop
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
              type="date"
              data-testid="ticket-due"
              aria-label="Due date"
              class="rounded-control border border-line bg-surface px-3 py-2 text-sm text-ink"
            >
          </label>
          <label class="flex flex-col gap-1 text-sm text-ink-muted">
            Milestone
            <select
              v-model="issueDraft.milestoneId"
              data-testid="ticket-milestone"
              aria-label="Milestone"
              :class="FIELD_CLASS"
            >
              <option :value="null">
                None
              </option>
              <option
                v-for="milestone in availableMilestones"
                :key="milestone.id"
                :value="milestone.id"
              >
                {{ milestone.name }}
              </option>
            </select>
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
