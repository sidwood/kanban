<script setup lang="ts">
// The Ticket editor surface: pick a Project, create Tickets under
// each kind's schema — an Implementation attached to one Spec with
// its slice and story-linked criteria, a Bug quick-captured with
// title, actual behaviour, and reporter evidence, a Task with a
// title and an optional attachment — and read the Project's Tickets
// back. A listed Bug opens the qualification form that completes it
// and the facts form that records its vendor-neutral collections.
// Presentation only; every domain call goes through the generated
// clients in the ticket-editor and bug-editor stores
// (KAN-S4-US1 through KAN-S4-US3).
import { computed, inject, onMounted, ref } from 'vue'
import { kanbanTransportKey } from '../core/transport'
import { useProjectRegisterStore } from '../stores/project-register'
import { useSpecEditorStore } from '../stores/spec-editor'
import {
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
  type BugFactsDraft,
  type BugQualificationDraft,
} from '../stores/bug-editor'
import type { TicketRecord } from '@kanban/contracts'

const transport = inject(kanbanTransportKey)
const projects = useProjectRegisterStore()
const specs = useSpecEditorStore()
const editor = useTicketEditorStore()
const bugEditor = useBugEditorStore()

const pickedProjectId = ref<number | null>(null)
const draft = ref(blankTicketDraft())
const pickedBugId = ref<number | null>(null)
const qualificationDraft = ref<BugQualificationDraft>(blankBugQualificationDraft())
const factsDraft = ref<BugFactsDraft>(blankBugFactsDraft())

onMounted(() => {
  if (transport) {
    void projects.refresh(transport).then(() => {
      const first = projects.projects.find((project) => !project.archived) ?? projects.projects[0]
      if (first) {
        pickedProjectId.value = first.id
        void loadAll(first.id)
      }
    })
  }
})

// Load the picked Project's Tickets and the Specs an attachment can
// name.
async function loadAll(projectId: number): Promise<void> {
  await Promise.all([
    editor.refresh(transport!, projectId),
    specs.refresh(transport!, projectId),
  ])
}

async function switchProject(): Promise<void> {
  if (transport && pickedProjectId.value !== null) {
    await loadAll(pickedProjectId.value)
    pickedBugId.value = null
  }
}

// The code of the picked Project, for rendering Ticket identities.
const projectCode = computed(
  () => projects.projects.find((project) => project.id === pickedProjectId.value)?.code ?? '',
)

function ticketId(ticket: { number: number }): string {
  return `${projectCode.value}-T${ticket.number}`
}

function specId(spec: { number: number }): string {
  return `${projectCode.value}-S${spec.number}`
}

// One Ticket's display line: the slice an Implementation names, or
// the title a Bug or Task carries.
function summary(ticket: { slice?: string | null; title?: string | null }): string {
  return ticket.slice ?? ticket.title ?? ''
}

// The Bugs of the picked Project, the qualification badge a Bug row
// shows, and the Bug the qualification form edits.
const bugs = computed(() => editor.tickets.filter((ticket) => ticket.kind === 'bug'))

function bugBadge(ticket: TicketRecord): string {
  const severity = ticket.bug?.qualification?.severity
  return severity ?? 'unqualified'
}

const pickedBug = computed(() => bugs.value.find((bug) => bug.id === pickedBugId.value) ?? null)

// Seed the qualification and facts forms from one Bug's record:
// what stands, or the blank forms when nothing does.
function pickBug(id: number): void {
  pickedBugId.value = id
  const bug = bugs.value.find((entry) => entry.id === id)
  qualificationDraft.value = bug?.bug?.qualification
    ? {
        expectedBehaviour: bug.bug.qualification.expected_behaviour,
        reproduction: bug.bug.qualification.reproduction,
        environment: bug.bug.qualification.environment,
        severity: bug.bug.qualification.severity,
        frequency: bug.bug.qualification.frequency,
        affectedScope: bug.bug.qualification.affected_scope,
        risk: bug.bug.qualification.risk,
        criteria: bug.bug.qualification.criteria.map((criterion) => ({
          outcome: criterion.outcome,
          stories: criterion.stories.join(', '),
        })),
        verificationSteps: bug.bug.qualification.verification_steps.map(
          (step) => step.command,
        ),
      }
    : blankBugQualificationDraft()
  const facts = bug?.bug
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

// Creating follows the picked kind; a landed Ticket resets the form.
async function submitCreate(): Promise<void> {
  if (transport && pickedProjectId.value !== null) {
    const landed = await editor.create(transport, pickedProjectId.value, draft.value)
    if (landed) {
      draft.value = blankTicketDraft()
    }
  }
}

async function submitQualify(): Promise<void> {
  const bug = pickedBug.value
  if (transport && bug && pickedProjectId.value !== null) {
    const landed = await bugEditor.qualify(
      transport,
      bug.id,
      bug.version,
      qualificationDraft.value,
    )
    if (landed) {
      await editor.refresh(transport, pickedProjectId.value)
      pickBug(landed.id)
    }
  }
}

async function submitFacts(): Promise<void> {
  const bug = pickedBug.value
  if (transport && bug && pickedProjectId.value !== null) {
    const landed = await bugEditor.recordFacts(transport, bug.id, bug.version, factsDraft.value)
    if (landed) {
      await editor.refresh(transport, pickedProjectId.value)
      pickBug(landed.id)
    }
  }
}

function addCriterion(): void {
  draft.value.criteria.push({ outcome: '', stories: '' })
}

function removeCriterion(position: number): void {
  draft.value.criteria.splice(position, 1)
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

const kindLabels: Record<string, string> = {
  implementation: 'implementation',
  bug: 'bug',
  task: 'task',
}
</script>

<template>
  <main class="mx-auto flex min-h-screen max-w-4xl flex-col gap-6 p-8">
    <nav class="text-sm text-slate-500">
      <RouterLink
        to="/"
        class="hover:text-slate-900"
      >
        Kanban
      </RouterLink>
      <span aria-hidden="true"> / </span>
      <span class="text-slate-900">Tickets</span>
    </nav>

    <h1 class="text-3xl font-semibold tracking-tight">
      Create Tickets
    </h1>

    <label class="flex w-fit flex-col gap-1 text-sm text-slate-600">
      Project
      <select
        v-model="pickedProjectId"
        data-testid="ticket-project"
        aria-label="Project"
        class="rounded border border-slate-300 px-3 py-2 text-sm"
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

    <p
      v-if="editor.error"
      data-testid="ticket-error"
      role="alert"
      class="rounded border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-700"
    >
      {{ editor.error }}
    </p>

    <section
      v-if="editor.loaded"
      data-testid="ticket-list"
      class="flex flex-col gap-2"
    >
      <h2 class="text-sm font-semibold text-slate-700">
        Tickets
      </h2>
      <ul class="flex flex-col divide-y divide-slate-200 rounded border border-slate-200">
        <li
          v-for="ticket in editor.tickets"
          :key="ticket.id"
          :data-testid="`ticket-row-${ticket.id}`"
          class="flex flex-wrap items-center gap-3 px-4 py-3 text-sm"
        >
          <span class="rounded bg-slate-100 px-2 py-0.5 font-mono text-sm font-medium">
            {{ ticketId(ticket) }}
          </span>
          <span class="rounded bg-slate-100 px-2 py-0.5 text-xs text-slate-600">
            {{ kindLabels[ticket.kind] }}
          </span>
          <span
            class="rounded bg-slate-100 px-2 py-0.5 text-xs text-slate-600"
          >{{ ticket.priority }}</span>
          <span class="text-xs text-slate-500">{{ ticket.state }}</span>
          <span
            v-if="ticket.kind === 'bug'"
            :data-testid="`ticket-bug-severity-${ticket.id}`"
            class="rounded bg-slate-100 px-2 py-0.5 text-xs text-slate-600"
          >{{ bugBadge(ticket) }}</span>
          <span class="text-slate-800">{{ summary(ticket) }}</span>
        </li>
      </ul>
    </section>
    <p
      v-else-if="!editor.error"
      data-testid="ticket-loading"
    >
      Loading Tickets…
    </p>

    <section class="flex flex-col gap-4 rounded-lg border border-slate-200 p-4">
      <h2 class="text-sm font-semibold text-slate-700">
        New Ticket
      </h2>

      <form
        class="flex flex-col gap-3"
        @submit.prevent="submitCreate"
      >
        <div class="flex flex-wrap gap-3">
          <label class="flex flex-col gap-1 text-sm text-slate-600">
            Kind
            <select
              v-model="draft.kind"
              data-testid="ticket-kind"
              aria-label="Ticket kind"
              class="rounded border border-slate-300 px-3 py-2 text-sm"
            >
              <option
                v-for="kind in TICKET_KINDS"
                :key="kind"
                :value="kind"
              >
                {{ kindLabels[kind] }}
              </option>
            </select>
          </label>
          <label class="flex flex-col gap-1 text-sm text-slate-600">
            Priority
            <select
              v-model="draft.priority"
              data-testid="ticket-priority"
              aria-label="Priority"
              class="rounded border border-slate-300 px-3 py-2 text-sm"
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
          <label class="flex flex-col gap-1 text-sm text-slate-600">
            {{ draft.kind === 'implementation' ? 'Spec (required)' : 'Spec (optional)' }}
            <select
              v-model="draft.specId"
              data-testid="ticket-spec"
              aria-label="Attached Spec"
              class="rounded border border-slate-300 px-3 py-2 text-sm"
            >
              <option
                v-if="draft.kind !== 'implementation'"
                :value="null"
              >
                No Spec
              </option>
              <option
                v-for="entry in specs.specs"
                :key="entry.id"
                :value="entry.id"
              >
                {{ specId(entry) }} — {{ entry.name }}
              </option>
            </select>
          </label>
        </div>

        <label
          v-if="draft.kind !== 'implementation'"
          class="flex flex-col gap-1 text-sm text-slate-600"
        >
          Title
          <input
            v-model="draft.title"
            data-testid="ticket-title"
            aria-label="Ticket title"
            placeholder="What is incorrect or needed?"
            class="rounded border border-slate-300 px-3 py-2 text-sm"
          >
        </label>

        <template v-if="draft.kind === 'bug'">
          <label class="flex flex-col gap-1 text-sm text-slate-600">
            Actual behaviour — what happened
            <textarea
              v-model="draft.actualBehaviour"
              data-testid="ticket-bug-actual"
              aria-label="Actual behaviour"
              rows="2"
              placeholder="What did you see happen?"
              class="rounded border border-slate-300 px-3 py-2 text-sm"
            />
          </label>
          <label class="flex flex-col gap-1 text-sm text-slate-600">
            Reporter evidence — what you hold
            <textarea
              v-model="draft.reporterEvidence"
              data-testid="ticket-bug-evidence"
              aria-label="Reporter evidence"
              rows="2"
              placeholder="What evidence do you hold for it?"
              class="rounded border border-slate-300 px-3 py-2 text-sm"
            />
          </label>
        </template>

        <template v-else-if="draft.kind === 'implementation'">
          <label class="flex flex-col gap-1 text-sm text-slate-600">
            Slice description — the behaviour delivered end to end
            <textarea
              v-model="draft.slice"
              data-testid="ticket-slice"
              aria-label="Slice description"
              rows="2"
              placeholder="What behaviour does this slice deliver, end to end?"
              class="rounded border border-slate-300 px-3 py-2 text-sm"
            />
          </label>

          <fieldset
            data-testid="ticket-criteria"
            class="flex flex-col gap-2"
          >
            <legend class="text-sm font-medium text-slate-600">
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
                class="min-w-56 flex-1 rounded border border-slate-300 px-3 py-2 text-sm"
              >
              <input
                v-model="criterion.stories"
                :data-testid="`ticket-criterion-stories-${position}`"
                :aria-label="`Criterion ${position + 1} stories`"
                placeholder="Stories, for example CORE-S1-US1, CORE-S1-US2"
                class="min-w-56 flex-1 rounded border border-slate-300 px-3 py-2 text-sm"
              >
              <button
                :data-testid="`ticket-criterion-remove-${position}`"
                type="button"
                class="rounded border border-slate-300 px-2 py-1 text-xs hover:bg-slate-50"
                @click="removeCriterion(position)"
              >
                Remove
              </button>
            </div>
            <button
              data-testid="ticket-criterion-add"
              type="button"
              class="w-fit rounded border border-slate-300 px-3 py-1.5 text-sm hover:bg-slate-50"
              @click="addCriterion"
            >
              Add criterion
            </button>
          </fieldset>
        </template>

        <button
          type="submit"
          data-testid="ticket-create"
          class="w-fit rounded bg-slate-900 px-3 py-2 text-sm font-medium text-white hover:bg-slate-700"
        >
          Create Ticket
        </button>
      </form>
    </section>

    <section
      v-if="bugs.length > 0"
      data-testid="bug-editor"
      class="flex flex-col gap-4 rounded-lg border border-slate-200 p-4"
    >
      <h2 class="text-sm font-semibold text-slate-700">
        Qualify a Bug
      </h2>

      <p
        v-if="bugEditor.error"
        data-testid="bug-error"
        role="alert"
        class="rounded border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-700"
      >
        {{ bugEditor.error }}
      </p>

      <label class="flex w-fit flex-col gap-1 text-sm text-slate-600">
        Bug
        <select
          :value="pickedBugId ?? ''"
          data-testid="bug-pick"
          aria-label="Bug to qualify"
          class="rounded border border-slate-300 px-3 py-2 text-sm"
          @change="pickBug(Number(($event.target as HTMLSelectElement).value))"
        >
          <option
            value=""
            disabled
          >
            Pick a Bug
          </option>
          <option
            v-for="bug in bugs"
            :key="bug.id"
            :value="bug.id"
          >
            {{ ticketId(bug) }} — {{ bug.title }} ({{ bugBadge(bug) }})
          </option>
        </select>
      </label>

      <form
        v-if="pickedBug"
        class="flex flex-col gap-3"
        @submit.prevent="submitQualify"
      >
        <div class="flex flex-wrap gap-3">
          <label class="flex flex-col gap-1 text-sm text-slate-600">
            Severity
            <select
              v-model="qualificationDraft.severity"
              data-testid="bug-qualify-severity"
              aria-label="Severity"
              class="rounded border border-slate-300 px-3 py-2 text-sm"
            >
              <option
                v-for="severity in BUG_SEVERITIES"
                :key="severity"
                :value="severity"
              >
                {{ severity }}
              </option>
            </select>
          </label>
        </div>

        <label class="flex flex-col gap-1 text-sm text-slate-600">
          Expected behaviour
          <input
            v-model="qualificationDraft.expectedBehaviour"
            data-testid="bug-qualify-expected"
            aria-label="Expected behaviour"
            class="rounded border border-slate-300 px-3 py-2 text-sm"
          >
        </label>
        <label class="flex flex-col gap-1 text-sm text-slate-600">
          Reproduction or failing test
          <textarea
            v-model="qualificationDraft.reproduction"
            data-testid="bug-qualify-reproduction"
            aria-label="Reproduction or failing test"
            rows="2"
            class="rounded border border-slate-300 px-3 py-2 text-sm"
          />
        </label>
        <div class="flex flex-wrap gap-3">
          <label class="flex flex-1 flex-col gap-1 text-sm text-slate-600">
            Environment
            <input
              v-model="qualificationDraft.environment"
              data-testid="bug-qualify-environment"
              aria-label="Environment"
              class="rounded border border-slate-300 px-3 py-2 text-sm"
            >
          </label>
          <label class="flex flex-1 flex-col gap-1 text-sm text-slate-600">
            Frequency
            <input
              v-model="qualificationDraft.frequency"
              data-testid="bug-qualify-frequency"
              aria-label="Frequency"
              class="rounded border border-slate-300 px-3 py-2 text-sm"
            >
          </label>
        </div>
        <div class="flex flex-wrap gap-3">
          <label class="flex flex-1 flex-col gap-1 text-sm text-slate-600">
            Affected scope
            <input
              v-model="qualificationDraft.affectedScope"
              data-testid="bug-qualify-scope"
              aria-label="Affected scope"
              class="rounded border border-slate-300 px-3 py-2 text-sm"
            >
          </label>
          <label class="flex flex-1 flex-col gap-1 text-sm text-slate-600">
            Risk
            <input
              v-model="qualificationDraft.risk"
              data-testid="bug-qualify-risk"
              aria-label="Risk"
              class="rounded border border-slate-300 px-3 py-2 text-sm"
            >
          </label>
        </div>

        <fieldset
          data-testid="bug-qualify-criteria"
          class="flex flex-col gap-2"
        >
          <legend class="text-sm font-medium text-slate-600">
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
              class="min-w-56 flex-1 rounded border border-slate-300 px-3 py-2 text-sm"
            >
            <input
              v-model="criterion.stories"
              :data-testid="`bug-qualify-criterion-stories-${position}`"
              :aria-label="`Criterion ${position + 1} stories`"
              placeholder="Stories, for example CORE-S1-US1, CORE-S1-US2"
              class="min-w-56 flex-1 rounded border border-slate-300 px-3 py-2 text-sm"
            >
            <button
              :data-testid="`bug-qualify-criterion-remove-${position}`"
              type="button"
              class="rounded border border-slate-300 px-2 py-1 text-xs hover:bg-slate-50"
              @click="removeQualificationCriterion(position)"
            >
              Remove
            </button>
          </div>
          <button
            data-testid="bug-qualify-criterion-add"
            type="button"
            class="w-fit rounded border border-slate-300 px-3 py-1.5 text-sm hover:bg-slate-50"
            @click="addQualificationCriterion"
          >
            Add criterion
          </button>
        </fieldset>

        <fieldset
          data-testid="bug-qualify-steps"
          class="flex flex-col gap-2"
        >
          <legend class="text-sm font-medium text-slate-600">
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
              class="min-w-56 flex-1 rounded border border-slate-300 px-3 py-2 text-sm"
            >
            <button
              :data-testid="`bug-qualify-step-remove-${position}`"
              type="button"
              class="rounded border border-slate-300 px-2 py-1 text-xs hover:bg-slate-50"
              @click="removeStep(position)"
            >
              Remove
            </button>
          </div>
          <button
            data-testid="bug-qualify-step-add"
            type="button"
            class="w-fit rounded border border-slate-300 px-3 py-1.5 text-sm hover:bg-slate-50"
            @click="addStep"
          >
            Add step
          </button>
        </fieldset>

        <button
          type="submit"
          data-testid="bug-qualify"
          class="w-fit rounded bg-slate-900 px-3 py-2 text-sm font-medium text-white hover:bg-slate-700"
        >
          Qualify Bug
        </button>
      </form>

      <form
        v-if="pickedBug"
        class="flex flex-col gap-3 border-t border-slate-200 pt-4"
        @submit.prevent="submitFacts"
      >
        <h3 class="text-sm font-semibold text-slate-700">
          Bug facts — External References, Occurrence Snapshots, Evidence Items
        </h3>

        <fieldset
          data-testid="bug-facts-references"
          class="flex flex-col gap-2"
        >
          <legend class="text-sm font-medium text-slate-600">
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
              class="min-w-56 flex-1 rounded border border-slate-300 px-3 py-2 text-sm"
            >
            <input
              v-model="reference.label"
              :data-testid="`bug-facts-reference-label-${position}`"
              :aria-label="`Reference ${position + 1} label`"
              placeholder="Label (optional)"
              class="min-w-40 flex-1 rounded border border-slate-300 px-3 py-2 text-sm"
            >
            <button
              :data-testid="`bug-facts-reference-remove-${position}`"
              type="button"
              class="rounded border border-slate-300 px-2 py-1 text-xs hover:bg-slate-50"
              @click="removeReference(position)"
            >
              Remove
            </button>
          </div>
          <button
            data-testid="bug-facts-reference-add"
            type="button"
            class="w-fit rounded border border-slate-300 px-3 py-1.5 text-sm hover:bg-slate-50"
            @click="addReference"
          >
            Add reference
          </button>
        </fieldset>

        <fieldset
          data-testid="bug-facts-snapshots"
          class="flex flex-col gap-2"
        >
          <legend class="text-sm font-medium text-slate-600">
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
              class="min-w-48 flex-1 rounded border border-slate-300 px-3 py-2 text-sm"
            >
            <input
              v-model="snapshot.observation"
              :data-testid="`bug-facts-snapshot-observation-${position}`"
              :aria-label="`Snapshot ${position + 1} observation`"
              placeholder="What was observed"
              class="min-w-56 flex-1 rounded border border-slate-300 px-3 py-2 text-sm"
            >
            <button
              :data-testid="`bug-facts-snapshot-remove-${position}`"
              type="button"
              class="rounded border border-slate-300 px-2 py-1 text-xs hover:bg-slate-50"
              @click="removeSnapshot(position)"
            >
              Remove
            </button>
          </div>
          <button
            data-testid="bug-facts-snapshot-add"
            type="button"
            class="w-fit rounded border border-slate-300 px-3 py-1.5 text-sm hover:bg-slate-50"
            @click="addSnapshot"
          >
            Add snapshot
          </button>
        </fieldset>

        <label class="flex flex-col gap-1 text-sm text-slate-600">
          Evidence Item identities — attached to this Bug
          <input
            v-model="factsDraft.evidenceIds"
            data-testid="bug-facts-evidence"
            aria-label="Evidence Item identities"
            placeholder="Identities, for example 2, 5"
            class="rounded border border-slate-300 px-3 py-2 text-sm"
          >
        </label>

        <button
          type="submit"
          data-testid="bug-facts"
          class="w-fit rounded bg-slate-900 px-3 py-2 text-sm font-medium text-white hover:bg-slate-700"
        >
          Record Bug facts
        </button>
      </form>
    </section>
  </main>
</template>
