<script setup lang="ts">
// The Ticket editor surface: pick a Project, create Tickets under
// each kind's schema — an Implementation attached to one Spec with
// its slice and story-linked criteria, a Bug or Task with a title and
// an optional attachment — and read the Project's Tickets back.
// Presentation only; every domain call goes through the generated
// client in the ticket-editor store (KAN-S4-US1, KAN-S4-US2).
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

const transport = inject(kanbanTransportKey)
const projects = useProjectRegisterStore()
const specs = useSpecEditorStore()
const editor = useTicketEditorStore()

const pickedProjectId = ref<number | null>(null)
const draft = ref(blankTicketDraft())

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

// Creating follows the picked kind; a landed Ticket resets the form.
async function submitCreate(): Promise<void> {
  if (transport && pickedProjectId.value !== null) {
    const landed = await editor.create(transport, pickedProjectId.value, draft.value)
    if (landed) {
      draft.value = blankTicketDraft()
    }
  }
}

function addCriterion(): void {
  draft.value.criteria.push({ outcome: '', stories: '' })
}

function removeCriterion(position: number): void {
  draft.value.criteria.splice(position, 1)
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

        <template v-else>
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
  </main>
</template>
