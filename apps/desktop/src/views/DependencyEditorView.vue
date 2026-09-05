<script setup lang="ts">
// The dependency editor surface: pick a Project and one of its
// Tickets, then wire what the Ticket waits on — registered
// dependencies, which may name Tickets of any registered Project,
// and the explicit external blockers that carry unregistered waiting
// work. The readiness panel shows the projection the core computes;
// this view never derives readiness itself (KAN-S4-US5).
import { computed, inject, onMounted, ref } from 'vue'
import type { TicketReadinessBlocker } from '@kanban/contracts'
import { kanbanTransportKey } from '../core/transport'
import { useProjectRegisterStore } from '../stores/project-register'
import { useDependencyEditorStore } from '../stores/dependency-editor'

const transport = inject(kanbanTransportKey)
const projects = useProjectRegisterStore()
const editor = useDependencyEditorStore()

const pickedProjectId = ref<number | null>(null)
const pickedTicketId = ref<number | null>(null)
const sourceProjectId = ref<number | null>(null)
const sourceTicketId = ref<number | null>(null)
const blockerDescription = ref('')

onMounted(() => {
  if (transport) {
    void projects.refresh(transport).then(() => {
      const first = projects.projects.find((project) => !project.archived) ?? projects.projects[0]
      if (first) {
        pickedProjectId.value = first.id
        sourceProjectId.value = first.id
        void loadTickets(first.id)
        void editor.loadSource(transport, first.id)
      }
    })
  }
})

// Load the picked Project's Tickets for the waiting-Ticket picker.
async function loadTickets(projectId: number): Promise<void> {
  pickedTicketId.value = null
  await editor.refresh(transport!, projectId)
}

async function switchProject(): Promise<void> {
  if (pickedProjectId.value !== null) {
    await loadTickets(pickedProjectId.value)
  }
}

// Open one Ticket's dependencies and readiness.
async function openTicket(ticketId: number): Promise<void> {
  pickedTicketId.value = ticketId
  await editor.open(transport!, ticketId)
}

async function switchSourceProject(): Promise<void> {
  sourceTicketId.value = null
  if (sourceProjectId.value !== null) {
    await editor.loadSource(transport!, sourceProjectId.value)
  }
}

// The code of one Project, for rendering Ticket identities.
const codes = computed(() => {
  const byId = new Map<number, string>()
  for (const project of projects.projects) {
    byId.set(project.id, project.code)
  }
  return byId
})

function ticketId(projectId: number, number: number): string {
  return `${codes.value.get(projectId) ?? '?'}-T${number}`
}

// One line per readiness blocker: the blocking Ticket's identity and
// state, or the external blocker's description.
function blockerLine(blocker: TicketReadinessBlocker): string {
  if ('Ticket' in blocker) {
    return `${ticketId(blocker.Ticket.from_project_id, blocker.Ticket.from_number)} — ${blocker.Ticket.from_state}`
  }
  return blocker.External.description
}

async function submitDependency(): Promise<void> {
  if (transport && pickedTicketId.value !== null && sourceTicketId.value !== null) {
    const landed = await editor.addDependency(transport, pickedTicketId.value, sourceTicketId.value)
    if (landed) {
      sourceTicketId.value = null
    }
  }
}

async function submitBlocker(): Promise<void> {
  if (transport && pickedTicketId.value !== null && blockerDescription.value.trim() !== '') {
    const landed = await editor.addBlocker(transport, pickedTicketId.value, blockerDescription.value)
    if (landed) {
      blockerDescription.value = ''
    }
  }
}

async function removeDependency(fromTicketId: number): Promise<void> {
  if (transport && pickedTicketId.value !== null) {
    await editor.removeDependency(transport, pickedTicketId.value, fromTicketId)
  }
}

async function removeBlocker(blockerId: number): Promise<void> {
  if (transport && pickedTicketId.value !== null) {
    await editor.removeBlocker(transport, pickedTicketId.value, blockerId)
  }
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
      <span class="text-slate-900">Dependencies</span>
    </nav>

    <h1 class="text-3xl font-semibold tracking-tight">
      Wire Dependencies
    </h1>

    <div class="flex flex-wrap items-end gap-3">
      <label class="flex flex-col gap-1 text-sm text-slate-600">
        Project
        <select
          v-model="pickedProjectId"
          data-testid="dependency-project"
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
      <label class="flex flex-col gap-1 text-sm text-slate-600">
        Ticket
        <select
          v-model="pickedTicketId"
          data-testid="dependency-ticket"
          aria-label="Ticket"
          class="rounded border border-slate-300 px-3 py-2 text-sm"
          @change="pickedTicketId !== null && openTicket(pickedTicketId)"
        >
          <option :value="null">
            Pick a Ticket
          </option>
          <option
            v-for="entry in editor.tickets"
            :key="entry.id"
            :value="entry.id"
          >
            {{ ticketId(entry.project_id, entry.number) }} — {{ entry.title ?? entry.slice }}
          </option>
        </select>
      </label>
    </div>

    <p
      v-if="editor.error"
      data-testid="dependency-error"
      role="alert"
      class="rounded border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-700"
    >
      {{ editor.error }}
    </p>

    <section
      v-if="editor.readiness"
      data-testid="dependency-readiness"
      class="flex flex-col gap-2 rounded-lg border border-slate-200 p-4"
    >
      <h2 class="text-sm font-semibold text-slate-700">
        Readiness
      </h2>
      <p
        data-testid="dependency-readiness-state"
        class="text-sm"
        :class="editor.readiness.ready ? 'text-emerald-700' : 'text-amber-700'"
      >
        {{ editor.readiness.ready ? 'Ready — nothing holds this Ticket back' : 'Waiting on' }}
      </p>
      <ul
        v-if="!editor.readiness.ready"
        class="flex flex-col gap-1 text-sm text-slate-700"
      >
        <li
          v-for="(blocker, position) in editor.readiness.blocked_by"
          :key="position"
          :data-testid="`dependency-blocker-${position}`"
        >
          {{ blockerLine(blocker) }}
        </li>
      </ul>
    </section>

    <section
      v-if="editor.dependencies"
      data-testid="dependency-list"
      class="flex flex-col gap-4"
    >
      <div class="flex flex-col gap-2">
        <h2 class="text-sm font-semibold text-slate-700">
          Registered dependencies
        </h2>
        <ul class="flex flex-col divide-y divide-slate-200 rounded border border-slate-200">
          <li
            v-for="dependency in editor.dependencies.dependencies"
            :key="dependency.from_ticket_id"
            :data-testid="`dependency-row-${dependency.from_ticket_id}`"
            class="flex flex-wrap items-center gap-3 px-4 py-3 text-sm"
          >
            <span class="rounded bg-slate-100 px-2 py-0.5 font-mono text-sm font-medium">
              {{ ticketId(dependency.from_project_id, dependency.from_number) }}
            </span>
            <span class="text-xs text-slate-500">{{ dependency.from_state }}</span>
            <span class="text-slate-800">must land first</span>
            <button
              :data-testid="`dependency-remove-${dependency.from_ticket_id}`"
              type="button"
              class="ml-auto rounded border border-slate-300 px-2 py-1 text-xs hover:bg-slate-50"
              @click="removeDependency(dependency.from_ticket_id)"
            >
              Remove
            </button>
          </li>
          <li
            v-if="editor.dependencies.dependencies.length === 0"
            class="px-4 py-3 text-sm text-slate-500"
          >
            No registered dependency
          </li>
        </ul>
      </div>

      <div class="flex flex-col gap-2">
        <h2 class="text-sm font-semibold text-slate-700">
          External blockers
        </h2>
        <ul class="flex flex-col divide-y divide-slate-200 rounded border border-slate-200">
          <li
            v-for="blocker in editor.dependencies.blockers"
            :key="blocker.id"
            :data-testid="`blocker-row-${blocker.id}`"
            class="flex flex-wrap items-center gap-3 px-4 py-3 text-sm"
          >
            <span class="text-slate-800">{{ blocker.description }}</span>
            <button
              :data-testid="`blocker-remove-${blocker.id}`"
              type="button"
              class="ml-auto rounded border border-slate-300 px-2 py-1 text-xs hover:bg-slate-50"
              @click="removeBlocker(blocker.id)"
            >
              Remove
            </button>
          </li>
          <li
            v-if="editor.dependencies.blockers.length === 0"
            class="px-4 py-3 text-sm text-slate-500"
          >
            No external blocker
          </li>
        </ul>
      </div>
    </section>

    <section
      v-if="pickedTicketId !== null"
      class="flex flex-col gap-4 rounded-lg border border-slate-200 p-4"
    >
      <h2 class="text-sm font-semibold text-slate-700">
        Add a dependency
      </h2>
      <form
        class="flex flex-wrap items-end gap-3"
        @submit.prevent="submitDependency"
      >
        <label class="flex flex-col gap-1 text-sm text-slate-600">
          Blocking Ticket's Project
          <select
            v-model="sourceProjectId"
            data-testid="dependency-source-project"
            aria-label="Blocking Ticket's Project"
            class="rounded border border-slate-300 px-3 py-2 text-sm"
            @change="switchSourceProject"
          >
            <option
              v-for="entry in projects.projects"
              :key="entry.id"
              :value="entry.id"
            >
              {{ entry.code }}
            </option>
          </select>
        </label>
        <label class="flex flex-col gap-1 text-sm text-slate-600">
          Blocking Ticket
          <select
            v-model="sourceTicketId"
            data-testid="dependency-source-ticket"
            aria-label="Blocking Ticket"
            class="rounded border border-slate-300 px-3 py-2 text-sm"
          >
            <option :value="null">
              Pick a Ticket
            </option>
            <option
              v-for="entry in editor.sourceTickets"
              :key="entry.id"
              :value="entry.id"
            >
              {{ ticketId(entry.project_id, entry.number) }} — {{ entry.title ?? entry.slice }}
            </option>
          </select>
        </label>
        <button
          type="submit"
          data-testid="dependency-add"
          class="rounded bg-slate-900 px-3 py-2 text-sm font-medium text-white hover:bg-slate-700"
        >
          Depends on
        </button>
      </form>

      <h2 class="text-sm font-semibold text-slate-700">
        Add an external blocker
      </h2>
      <form
        class="flex flex-wrap items-end gap-3"
        @submit.prevent="submitBlocker"
      >
        <label class="flex flex-1 flex-col gap-1 text-sm text-slate-600">
          Unregistered waiting work
          <input
            v-model="blockerDescription"
            data-testid="blocker-description"
            aria-label="External blocker description"
            placeholder="What is this Ticket waiting on?"
            class="rounded border border-slate-300 px-3 py-2 text-sm"
          >
        </label>
        <button
          type="submit"
          data-testid="blocker-add"
          class="rounded bg-slate-900 px-3 py-2 text-sm font-medium text-white hover:bg-slate-700"
        >
          Block on
        </button>
      </form>
    </section>
  </main>
</template>
