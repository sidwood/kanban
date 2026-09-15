<script setup lang="ts">
// The one Ticket editor: a kind-adaptive dialog the operator reaches
// from New Ticket, from a Spec's uncovered Story, and from the
// drawer's Edit, preset to the kind that entry point means
// (KAN-S4-US1, KAN-S4-US3). Creating sends `ticket.create`; revising
// sends `ticket.edit`, and a Bug's qualification and vendor-neutral
// facts send `ticket.bug.qualify` and `ticket.bug.facts` — each at the
// version the record was read at, each refusal reported rather than
// swallowed. Presentation only: the core decides what a kind may
// carry and what a complete qualification is, and a command can only
// ever address the Ticket this dialog was opened on.
import { computed, inject, ref, watch } from 'vue'
import { KanbanClient } from '@kanban/contracts'
import type { SpecRecord, TicketRecord } from '@kanban/contracts'
import { asApiError, kanbanTransportKey } from '../core/transport'
import { useProjectRegisterStore } from '../stores/project-register'
import { useTicketDialogStore } from '../stores/ticket-dialog'
import type { TicketEditorRequest } from '../stores/ticket-dialog'
import {
  TASK_MODES,
  TASK_SUBTYPES,
  TICKET_KINDS,
  TICKET_PRIORITIES,
  blankTicketDraft,
  useTicketEditorStore,
} from '../stores/ticket-editor'
import {
  BUG_SEVERITIES,
  blankBugFactsDraft,
  blankBugQualificationDraft,
  useBugEditorStore,
  withChosenSeverity,
  type BugFactsDraft,
  type BugQualificationDraft,
} from '../stores/bug-editor'
import AppButton from './AppButton.vue'
import AppDialog from './AppDialog.vue'
import InlineAlert from './InlineAlert.vue'

const transport = inject(kanbanTransportKey)
const dialog = useTicketDialogStore()
const projects = useProjectRegisterStore()
const editor = useTicketEditorStore()
const bugEditor = useBugEditorStore()

const projectId = ref<number | null>(null)
const specs = ref<SpecRecord[]>([])
const record = ref<TicketRecord | null>(null)
const draft = ref(blankTicketDraft())
const qualificationDraft = ref<BugQualificationDraft>(blankBugQualificationDraft())
const factsDraft = ref<BugFactsDraft>(blankBugFactsDraft())
const readError = ref<string | null>(null)
const saved = ref(false)

// Every read carries the opening it was issued under, so a dialog
// the operator has closed or re-pointed never renders an answer
// belonging to the record it left.
let opening = 0

const open = computed(() => dialog.editorOpen)
const mode = computed(() => dialog.editor?.mode ?? 'create')
const kindLocked = computed(() => dialog.editor?.kindLocked ?? false)
const error = computed(() => readError.value ?? editor.error)

const KIND_LABELS: Record<string, string> = {
  implementation: 'Implementation',
  bug: 'Bug',
  task: 'Task',
}

const title = computed(() =>
  mode.value === 'create'
    ? `New ${KIND_LABELS[draft.value.kind]} Ticket`
    : `Edit ${KIND_LABELS[draft.value.kind]} Ticket`,
)

const project = computed(
  () => projects.projects.find((entry) => entry.id === projectId.value) ?? null,
)

const identity = computed(() =>
  record.value && project.value ? `${project.value.code}-T${record.value.number}` : null,
)

function specIdOf(spec: SpecRecord): string {
  return `${project.value?.code ?? ''}-S${spec.number}`
}

// A Bug's qualification is a whole act: the typed command cannot be
// built without the severity the operator chose, so the form says so
// rather than sending a half-qualified Bug (DR-TK-09, DR-LC-13).
const qualificationGap = computed(() =>
  qualificationDraft.value.severity === null ? 'a chosen severity' : null,
)

// The store replaces the request whole on every entry point, so
// identity is the signal: a re-point re-reads, an unrelated keystroke
// does not.
watch(() => dialog.editor, (request) => void adopt(request), { immediate: true })

async function adopt(request: TicketEditorRequest | null): Promise<void> {
  opening += 1
  const attempt = opening
  record.value = null
  specs.value = []
  readError.value = null
  editor.error = null
  bugEditor.error = null
  saved.value = false
  if (!request || !transport) return

  draft.value = blankTicketDraft()
  draft.value.kind = request.kind
  draft.value.specId = request.specId
  if (request.story !== null) {
    draft.value.criteria = [{ outcome: '', stories: request.story }]
  }
  qualificationDraft.value = blankBugQualificationDraft()
  factsDraft.value = blankBugFactsDraft()
  projectId.value = request.projectId

  try {
    if (!projects.loaded) await projects.refresh(transport)
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
      seed(held)
    }
    await loadSpecs(attempt)
  } catch (failure) {
    if (attempt !== opening) return
    readError.value = asApiError(failure).message
  }
}

// Seed every form from the record the core holds, never from a draft
// a previous open left behind.
function seed(held: TicketRecord): void {
  draft.value.kind = held.kind
  draft.value.priority = held.priority
  draft.value.specId = held.spec_id ?? null
  draft.value.title = held.title ?? ''
  draft.value.slice = held.slice ?? ''
  draft.value.criteria = held.criteria.map((criterion) => ({
    outcome: criterion.outcome,
    stories: criterion.stories.join(', '),
  }))
  draft.value.subtype = held.subtype ?? draft.value.subtype
  draft.value.mode = held.mode ?? draft.value.mode
  draft.value.completion = [...held.completion]
  draft.value.actualBehaviour = held.bug?.actual_behaviour ?? ''
  draft.value.reporterEvidence = held.bug?.reporter_evidence ?? ''
  const qualification = held.bug?.qualification
  qualificationDraft.value = qualification
    ? {
        expectedBehaviour: qualification.expected_behaviour,
        reproduction: qualification.reproduction,
        environment: qualification.environment,
        severity: qualification.severity,
        frequency: qualification.frequency,
        affectedScope: qualification.affected_scope,
        risk: qualification.risk,
        criteria: qualification.criteria.map((criterion) => ({
          outcome: criterion.outcome,
          stories: criterion.stories.join(', '),
        })),
        verificationSteps: qualification.verification_steps.map((step) => step.command),
      }
    : blankBugQualificationDraft()
  const facts = held.bug
  factsDraft.value = {
    externalReferences: facts?.external_references.length
      ? facts.external_references.map((reference) => ({
          uri: reference.uri,
          label: reference.label ?? '',
        }))
      : [{ uri: '', label: '' }],
    occurrenceSnapshots: facts?.occurrence_snapshots.length
      ? facts.occurrence_snapshots.map((snapshot) => ({
          observedAt: snapshot.observed_at,
          observation: snapshot.observation,
        }))
      : [{ observedAt: '', observation: '' }],
    evidenceIds: facts?.evidence_ids.join(', ') ?? '',
  }
}

async function loadSpecs(attempt: number): Promise<void> {
  if (!transport || projectId.value === null) return
  const response = await new KanbanClient(transport).querySpecList({
    project_id: projectId.value,
  })
  // An answer for a Project the dialog has left renders nowhere.
  if (attempt !== opening) return
  specs.value = response.specs
}

// The Project is the Ticket's own fact once one exists, so only a
// creation may change it — and changing it takes the previous
// Project's Specs away before anything can name one.
async function switchProject(): Promise<void> {
  opening += 1
  const attempt = opening
  specs.value = []
  draft.value.specId = null
  readError.value = null
  try {
    await loadSpecs(attempt)
  } catch (failure) {
    if (attempt !== opening) return
    readError.value = asApiError(failure).message
  }
}

async function submitCreate(): Promise<void> {
  if (!transport || projectId.value === null) return
  const landed = await editor.create(transport, projectId.value, draft.value)
  if (landed) dialog.closeEditor()
}

async function submitEdit(): Promise<void> {
  const held = record.value
  if (!transport || !held) return
  const landed = await editor.edit(
    transport,
    held.id,
    held.version,
    held.kind === 'implementation'
      ? { slice: draft.value.slice }
      : { title: draft.value.title },
  )
  if (landed) {
    record.value = landed
    saved.value = true
  }
}

async function submitQualify(): Promise<void> {
  const held = record.value
  const chosen = withChosenSeverity(qualificationDraft.value)
  if (!transport || !held || !chosen) return
  const landed = await bugEditor.qualify(transport, held.id, held.version, chosen)
  if (landed) {
    record.value = landed
    seed(landed)
  }
}

async function submitFacts(): Promise<void> {
  const held = record.value
  if (!transport || !held) return
  const landed = await bugEditor.recordFacts(transport, held.id, held.version, factsDraft.value)
  if (landed) {
    record.value = landed
    seed(landed)
  }
}

function addCriterion(): void {
  draft.value.criteria.push({ outcome: '', stories: '' })
}
function removeCriterion(position: number): void {
  draft.value.criteria.splice(position, 1)
}
function addCompletion(): void {
  draft.value.completion.push('')
}
function removeCompletion(position: number): void {
  draft.value.completion.splice(position, 1)
}
function addQualificationCriterion(): void {
  qualificationDraft.value.criteria.push({ outcome: '', stories: '' })
}
function removeQualificationCriterion(position: number): void {
  qualificationDraft.value.criteria.splice(position, 1)
}
function addStep(): void {
  qualificationDraft.value.verificationSteps.push('')
}
function removeStep(position: number): void {
  qualificationDraft.value.verificationSteps.splice(position, 1)
}
function addReference(): void {
  factsDraft.value.externalReferences.push({ uri: '', label: '' })
}
function removeReference(position: number): void {
  factsDraft.value.externalReferences.splice(position, 1)
}
function addSnapshot(): void {
  factsDraft.value.occurrenceSnapshots.push({ observedAt: '', observation: '' })
}
function removeSnapshot(position: number): void {
  factsDraft.value.occurrenceSnapshots.splice(position, 1)
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
      v-if="identity"
      #subtitle
    >
      {{ identity }} · version {{ record?.version }}
    </template>

    <InlineAlert
      v-if="error"
      data-testid="ticket-error"
    >
      {{ error }}
    </InlineAlert>

    <div class="flex flex-wrap gap-3">
      <label class="flex flex-col gap-1 text-sm text-ink-muted">
        Project
        <select
          v-model="projectId"
          data-testid="ticket-project"
          aria-label="Project"
          :disabled="mode === 'edit'"
          :class="FIELD_CLASS"
          @change="switchProject"
        >
          <option
            v-for="entry in projects.projects"
            :key="entry.id"
            :value="entry.id"
          >
            {{ entry.code }} — {{ entry.name }}{{ entry.archived ? ' (archived)' : '' }}
          </option>
        </select>
      </label>

      <label class="flex flex-col gap-1 text-sm text-ink-muted">
        Kind
        <select
          v-model="draft.kind"
          data-testid="ticket-kind"
          aria-label="Ticket kind"
          :disabled="kindLocked"
          :class="FIELD_CLASS"
        >
          <option
            v-for="kind in TICKET_KINDS"
            :key="kind"
            :value="kind"
          >
            {{ KIND_LABELS[kind] }}
          </option>
        </select>
      </label>

      <label
        v-if="mode === 'create'"
        class="flex flex-col gap-1 text-sm text-ink-muted"
      >
        Priority
        <select
          v-model="draft.priority"
          data-testid="ticket-priority"
          aria-label="Priority"
          :class="FIELD_CLASS"
        >
          <option
            v-for="priority in TICKET_PRIORITIES"
            :key="priority"
            :value="priority"
          >
            {{ priority }}
          </option>
        </select>
      </label>
      <p
        v-else
        data-testid="ticket-priority-fact"
        class="self-end text-sm text-ink-muted"
      >
        Priority {{ draft.priority }}
      </p>

      <label
        v-if="mode === 'create'"
        class="flex flex-col gap-1 text-sm text-ink-muted"
      >
        {{ draft.kind === 'implementation' ? 'Spec (required)' : 'Spec (optional)' }}
        <select
          v-model="draft.specId"
          data-testid="ticket-spec"
          aria-label="Attached Spec"
          :class="FIELD_CLASS"
        >
          <option
            v-if="draft.kind !== 'implementation'"
            :value="null"
          >
            No Spec
          </option>
          <option
            v-for="entry in specs"
            :key="entry.id"
            :value="entry.id"
          >
            {{ specIdOf(entry) }} — {{ entry.name }}
          </option>
        </select>
      </label>
    </div>

    <form
      class="flex flex-col gap-3"
      @submit.prevent="mode === 'create' ? submitCreate() : submitEdit()"
    >
      <label
        v-if="draft.kind !== 'implementation'"
        class="flex flex-col gap-1 text-sm text-ink-muted"
      >
        Title
        <input
          v-model="draft.title"
          data-testid="ticket-title"
          aria-label="Ticket title"
          required
          placeholder="What is incorrect or needed?"
          :class="FIELD_CLASS"
        >
      </label>

      <template v-if="draft.kind === 'implementation'">
        <label class="flex flex-col gap-1 text-sm text-ink-muted">
          Slice description — the behaviour delivered end to end
          <textarea
            v-model="draft.slice"
            data-testid="ticket-slice"
            aria-label="Slice description"
            rows="2"
            required
            placeholder="What behaviour does this slice deliver, end to end?"
            :class="FIELD_CLASS"
          />
        </label>

        <fieldset
          v-if="mode === 'create'"
          data-testid="ticket-criteria"
          class="flex flex-col gap-2"
        >
          <legend class="text-sm font-medium text-ink-muted">
            Story-linked criteria
          </legend>
          <div
            v-for="(criterion, position) in draft.criteria"
            :key="position"
            class="flex flex-wrap items-center gap-2"
          >
            <input
              v-model="criterion.outcome"
              :data-testid="`ticket-criterion-outcome-${position}`"
              :aria-label="`Criterion ${position + 1} outcome`"
              placeholder="The observable outcome"
              class="min-w-56 flex-1 rounded-control border border-line bg-surface px-3 py-2 text-sm text-ink"
            >
            <input
              v-model="criterion.stories"
              :data-testid="`ticket-criterion-stories-${position}`"
              :aria-label="`Criterion ${position + 1} stories`"
              placeholder="Stories, for example CORE-S1-US1"
              class="min-w-56 flex-1 rounded-control border border-line bg-surface px-3 py-2 text-sm text-ink"
            >
            <AppButton
              size="sm"
              :data-testid="`ticket-criterion-remove-${position}`"
              @click="removeCriterion(position)"
            >
              Remove
            </AppButton>
          </div>
          <AppButton
            size="sm"
            class="w-fit"
            data-testid="ticket-criterion-add"
            @click="addCriterion"
          >
            Add criterion
          </AppButton>
        </fieldset>

        <section
          v-else
          data-testid="ticket-criteria-fact"
          class="flex flex-col gap-1 text-sm text-ink-muted"
        >
          <h3 class="text-xs font-semibold tracking-[0.06em] text-ink-subtle uppercase">
            Story-linked criteria
          </h3>
          <p
            v-for="(criterion, position) in draft.criteria"
            :key="position"
          >
            {{ criterion.outcome }} — {{ criterion.stories }}
          </p>
        </section>
      </template>

      <template v-if="draft.kind === 'bug'">
        <label class="flex flex-col gap-1 text-sm text-ink-muted">
          Actual behaviour — what happened
          <textarea
            v-model="draft.actualBehaviour"
            data-testid="ticket-bug-actual"
            aria-label="Actual behaviour"
            rows="2"
            :readonly="mode === 'edit'"
            required
            placeholder="What did you see happen?"
            :class="FIELD_CLASS"
          />
        </label>
        <label class="flex flex-col gap-1 text-sm text-ink-muted">
          Reporter evidence — what you hold
          <textarea
            v-model="draft.reporterEvidence"
            data-testid="ticket-bug-evidence"
            aria-label="Reporter evidence"
            rows="2"
            :readonly="mode === 'edit'"
            required
            placeholder="What evidence do you hold for it?"
            :class="FIELD_CLASS"
          />
        </label>
      </template>

      <template v-if="draft.kind === 'task'">
        <div class="flex flex-wrap gap-3">
          <label class="flex w-fit flex-col gap-1 text-sm text-ink-muted">
            Subtype
            <select
              v-model="draft.subtype"
              data-testid="ticket-subtype"
              aria-label="Task subtype"
              :disabled="mode === 'edit'"
              :class="FIELD_CLASS"
            >
              <option
                v-for="subtype in TASK_SUBTYPES"
                :key="subtype"
                :value="subtype"
              >
                {{ subtype }}
              </option>
            </select>
          </label>
          <label class="flex w-fit flex-col gap-1 text-sm text-ink-muted">
            Mode
            <select
              v-model="draft.mode"
              data-testid="ticket-mode"
              aria-label="Task mode"
              :disabled="mode === 'edit'"
              :class="FIELD_CLASS"
            >
              <option
                v-for="taskMode in TASK_MODES"
                :key="taskMode"
                :value="taskMode"
              >
                {{ taskMode }}
              </option>
            </select>
          </label>
        </div>

        <fieldset
          data-testid="ticket-completion"
          class="flex flex-col gap-2"
        >
          <legend class="text-sm font-medium text-ink-muted">
            Completion criteria
          </legend>
          <div
            v-for="(_, position) in draft.completion"
            :key="position"
            class="flex flex-wrap items-center gap-2"
          >
            <input
              v-model="draft.completion[position]"
              :data-testid="`ticket-completion-outcome-${position}`"
              :aria-label="`Completion criterion ${position + 1}`"
              :readonly="mode === 'edit'"
              placeholder="The outcome that bounds this Task"
              class="min-w-56 flex-1 rounded-control border border-line bg-surface px-3 py-2 text-sm text-ink"
            >
            <AppButton
              v-if="mode === 'create'"
              size="sm"
              :data-testid="`ticket-completion-remove-${position}`"
              @click="removeCompletion(position)"
            >
              Remove
            </AppButton>
          </div>
          <AppButton
            v-if="mode === 'create'"
            size="sm"
            class="w-fit"
            data-testid="ticket-completion-add"
            @click="addCompletion"
          >
            Add criterion
          </AppButton>
        </fieldset>

        <div
          v-if="mode === 'create'"
          class="flex flex-wrap gap-3"
        >
          <label class="flex w-fit flex-col gap-1 text-sm text-ink-muted">
            Scheduled for (optional)
            <input
              v-model="draft.scheduledFor"
              data-testid="ticket-scheduled-for"
              aria-label="One-time activation, RFC 3339"
              placeholder="2026-10-01T09:00:00Z"
              class="rounded-control border border-line bg-surface px-3 py-2 font-mono text-sm text-ink"
            >
          </label>
          <label class="flex w-fit flex-col gap-1 text-sm text-ink-muted">
            Due date (optional)
            <input
              v-model="draft.due"
              data-testid="ticket-due"
              aria-label="Due date, RFC 3339"
              placeholder="2026-09-30T17:00:00Z"
              class="rounded-control border border-line bg-surface px-3 py-2 font-mono text-sm text-ink"
            >
          </label>
        </div>
      </template>

      <div class="flex items-center gap-3">
        <AppButton
          v-if="mode === 'create'"
          variant="primary"
          type="submit"
          data-testid="ticket-create"
        >
          Create Ticket
        </AppButton>
        <AppButton
          v-else
          variant="primary"
          type="submit"
          data-testid="ticket-save"
        >
          Save Ticket
        </AppButton>
        <p
          v-if="saved"
          data-testid="ticket-saved"
          class="text-sm text-ink-muted"
        >
          Saved.
        </p>
      </div>
    </form>

    <!-- A Bug's qualification and its vendor-neutral facts are their
         own acts on the Bug this dialog was opened on, never on a
         Bug picked from a list that may have moved on. -->
    <template v-if="mode === 'edit' && record?.bug">
      <InlineAlert
        v-if="bugEditor.error"
        data-testid="bug-error"
      >
        {{ bugEditor.error }}
      </InlineAlert>

      <form
        class="flex flex-col gap-3 border-t border-line pt-4"
        @submit.prevent="submitQualify"
      >
        <h3 class="font-display text-sm font-semibold tracking-tight text-ink">
          Qualification
        </h3>

        <label class="flex w-fit flex-col gap-1 text-sm text-ink-muted">
          Severity
          <select
            v-model="qualificationDraft.severity"
            data-testid="bug-qualify-severity"
            aria-label="Severity"
            :class="FIELD_CLASS"
          >
            <option
              disabled
              :value="null"
            >
              Choose a severity
            </option>
            <option
              v-for="severity in BUG_SEVERITIES"
              :key="severity"
              :value="severity"
            >
              {{ severity }}
            </option>
          </select>
        </label>

        <label class="flex flex-col gap-1 text-sm text-ink-muted">
          Expected behaviour
          <input
            v-model="qualificationDraft.expectedBehaviour"
            data-testid="bug-qualify-expected"
            aria-label="Expected behaviour"
            :class="FIELD_CLASS"
          >
        </label>
        <label class="flex flex-col gap-1 text-sm text-ink-muted">
          Reproduction or failing test
          <textarea
            v-model="qualificationDraft.reproduction"
            data-testid="bug-qualify-reproduction"
            aria-label="Reproduction or failing test"
            rows="2"
            :class="FIELD_CLASS"
          />
        </label>
        <div class="flex flex-wrap gap-3">
          <label class="flex flex-1 flex-col gap-1 text-sm text-ink-muted">
            Environment
            <input
              v-model="qualificationDraft.environment"
              data-testid="bug-qualify-environment"
              aria-label="Environment"
              :class="FIELD_CLASS"
            >
          </label>
          <label class="flex flex-1 flex-col gap-1 text-sm text-ink-muted">
            Frequency
            <input
              v-model="qualificationDraft.frequency"
              data-testid="bug-qualify-frequency"
              aria-label="Frequency"
              :class="FIELD_CLASS"
            >
          </label>
        </div>
        <div class="flex flex-wrap gap-3">
          <label class="flex flex-1 flex-col gap-1 text-sm text-ink-muted">
            Affected scope
            <input
              v-model="qualificationDraft.affectedScope"
              data-testid="bug-qualify-scope"
              aria-label="Affected scope"
              :class="FIELD_CLASS"
            >
          </label>
          <label class="flex flex-1 flex-col gap-1 text-sm text-ink-muted">
            Risk
            <input
              v-model="qualificationDraft.risk"
              data-testid="bug-qualify-risk"
              aria-label="Risk"
              :class="FIELD_CLASS"
            >
          </label>
        </div>

        <fieldset
          data-testid="bug-qualify-criteria"
          class="flex flex-col gap-2"
        >
          <legend class="text-sm font-medium text-ink-muted">
            Story-linked criteria
          </legend>
          <div
            v-for="(criterion, position) in qualificationDraft.criteria"
            :key="position"
            class="flex flex-wrap items-center gap-2"
          >
            <input
              v-model="criterion.outcome"
              :data-testid="`bug-qualify-criterion-outcome-${position}`"
              :aria-label="`Criterion ${position + 1} outcome`"
              placeholder="The observable outcome"
              class="min-w-56 flex-1 rounded-control border border-line bg-surface px-3 py-2 text-sm text-ink"
            >
            <input
              v-model="criterion.stories"
              :data-testid="`bug-qualify-criterion-stories-${position}`"
              :aria-label="`Criterion ${position + 1} stories`"
              placeholder="Stories, for example CORE-S1-US1"
              class="min-w-56 flex-1 rounded-control border border-line bg-surface px-3 py-2 text-sm text-ink"
            >
            <AppButton
              size="sm"
              :data-testid="`bug-qualify-criterion-remove-${position}`"
              @click="removeQualificationCriterion(position)"
            >
              Remove
            </AppButton>
          </div>
          <AppButton
            size="sm"
            class="w-fit"
            data-testid="bug-qualify-criterion-add"
            @click="addQualificationCriterion"
          >
            Add criterion
          </AppButton>
        </fieldset>

        <fieldset
          data-testid="bug-qualify-steps"
          class="flex flex-col gap-2"
        >
          <legend class="text-sm font-medium text-ink-muted">
            Verification Steps
          </legend>
          <div
            v-for="(_, position) in qualificationDraft.verificationSteps"
            :key="position"
            class="flex flex-wrap items-center gap-2"
          >
            <input
              v-model="qualificationDraft.verificationSteps[position]"
              :data-testid="`bug-qualify-step-${position}`"
              :aria-label="`Verification Step ${position + 1}`"
              placeholder="The command that demonstrates the criteria"
              class="min-w-56 flex-1 rounded-control border border-line bg-surface px-3 py-2 text-sm text-ink"
            >
            <AppButton
              size="sm"
              :data-testid="`bug-qualify-step-remove-${position}`"
              @click="removeStep(position)"
            >
              Remove
            </AppButton>
          </div>
          <AppButton
            size="sm"
            class="w-fit"
            data-testid="bug-qualify-step-add"
            @click="addStep"
          >
            Add step
          </AppButton>
        </fieldset>

        <p
          v-if="qualificationGap"
          data-testid="bug-qualify-incomplete"
          class="text-sm text-caution"
        >
          Qualification needs {{ qualificationGap }} before it can be recorded.
        </p>

        <AppButton
          variant="primary"
          type="submit"
          class="w-fit"
          data-testid="bug-qualify"
          :disabled="qualificationGap !== null"
        >
          Qualify Bug
        </AppButton>
      </form>

      <form
        class="flex flex-col gap-3 border-t border-line pt-4"
        @submit.prevent="submitFacts"
      >
        <h3 class="font-display text-sm font-semibold tracking-tight text-ink">
          Bug facts — External References, Occurrence Snapshots, Evidence Items
        </h3>

        <fieldset
          data-testid="bug-facts-references"
          class="flex flex-col gap-2"
        >
          <legend class="text-sm font-medium text-ink-muted">
            External References
          </legend>
          <div
            v-for="(reference, position) in factsDraft.externalReferences"
            :key="position"
            class="flex flex-wrap items-center gap-2"
          >
            <input
              v-model="reference.uri"
              :data-testid="`bug-facts-reference-uri-${position}`"
              :aria-label="`Reference ${position + 1} URI`"
              placeholder="The URI, like https://example.invalid/issues/12"
              class="min-w-56 flex-1 rounded-control border border-line bg-surface px-3 py-2 text-sm text-ink"
            >
            <input
              v-model="reference.label"
              :data-testid="`bug-facts-reference-label-${position}`"
              :aria-label="`Reference ${position + 1} label`"
              placeholder="Label (optional)"
              class="min-w-40 flex-1 rounded-control border border-line bg-surface px-3 py-2 text-sm text-ink"
            >
            <AppButton
              size="sm"
              :data-testid="`bug-facts-reference-remove-${position}`"
              @click="removeReference(position)"
            >
              Remove
            </AppButton>
          </div>
          <AppButton
            size="sm"
            class="w-fit"
            data-testid="bug-facts-reference-add"
            @click="addReference"
          >
            Add reference
          </AppButton>
        </fieldset>

        <fieldset
          data-testid="bug-facts-snapshots"
          class="flex flex-col gap-2"
        >
          <legend class="text-sm font-medium text-ink-muted">
            Occurrence Snapshots
          </legend>
          <div
            v-for="(snapshot, position) in factsDraft.occurrenceSnapshots"
            :key="position"
            class="flex flex-wrap items-center gap-2"
          >
            <input
              v-model="snapshot.observedAt"
              :data-testid="`bug-facts-snapshot-at-${position}`"
              :aria-label="`Snapshot ${position + 1} observed at`"
              placeholder="Observed at, like 2026-09-05T07:41:00Z"
              class="min-w-48 flex-1 rounded-control border border-line bg-surface px-3 py-2 text-sm text-ink"
            >
            <input
              v-model="snapshot.observation"
              :data-testid="`bug-facts-snapshot-observation-${position}`"
              :aria-label="`Snapshot ${position + 1} observation`"
              placeholder="What was observed"
              class="min-w-56 flex-1 rounded-control border border-line bg-surface px-3 py-2 text-sm text-ink"
            >
            <AppButton
              size="sm"
              :data-testid="`bug-facts-snapshot-remove-${position}`"
              @click="removeSnapshot(position)"
            >
              Remove
            </AppButton>
          </div>
          <AppButton
            size="sm"
            class="w-fit"
            data-testid="bug-facts-snapshot-add"
            @click="addSnapshot"
          >
            Add snapshot
          </AppButton>
        </fieldset>

        <label class="flex flex-col gap-1 text-sm text-ink-muted">
          Evidence Item identities — attached to this Bug
          <input
            v-model="factsDraft.evidenceIds"
            data-testid="bug-facts-evidence"
            aria-label="Evidence Item identities"
            placeholder="Identities, for example 2, 5"
            :class="FIELD_CLASS"
          >
        </label>

        <AppButton
          variant="primary"
          type="submit"
          class="w-fit"
          data-testid="bug-facts"
        >
          Record Bug facts
        </AppButton>
      </form>
    </template>
  </AppDialog>
</template>
