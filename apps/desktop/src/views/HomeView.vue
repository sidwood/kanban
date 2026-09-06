<script setup lang="ts">
// The boot surface: everything the operator sees before the board
// views land, bound to connection state from the generated client.
// Selecting a Project mounts its timeline and rulings under the
// numeric identity the core resolves (KAN-S2-US1, KAN-T79).
import { computed, inject, onMounted, ref, watch } from 'vue'
import RulingsSurface from '../components/RulingsSurface.vue'
import TimelineSurface from '../components/TimelineSurface.vue'
import { kanbanTransportKey } from '../core/transport'
import { useConnectionStore } from '../stores/connection'
import { useProjectRegisterStore } from '../stores/project-register'

const transport = inject(kanbanTransportKey)
const connection = useConnectionStore()
const projects = useProjectRegisterStore()
const selectedProjectId = ref<number | null>(null)

const timelineScope = computed(() =>
  selectedProjectId.value === null ? null : { project: selectedProjectId.value },
)

onMounted(() => {
  if (transport) {
    void connection.boot(transport)
  }
})

watch(
  () => connection.phase,
  (phase) => {
    if (phase === 'connected' && transport) {
      void projects.refresh(transport)
    }
  },
  { immediate: true },
)

const status = computed(() => {
  switch (connection.phase) {
    case 'connected':
      return `Core connected · v${connection.serviceVersion ?? 'unknown'}`
    case 'disconnected':
      return 'Core unreachable'
    default:
      return 'Connecting to the core…'
  }
})

const eventStream = computed(() =>
  connection.lastEventSequence === null
    ? 'Event stream idle'
    : `Event stream live · sequence ${connection.lastEventSequence}`,
)
</script>

<template>
  <main class="flex min-h-screen flex-col items-center justify-center gap-3">
    <h1 class="text-4xl font-semibold tracking-tight">
      Kanban
    </h1>
    <p
      data-testid="connection-status"
      class="text-sm text-slate-600"
      aria-live="polite"
    >
      {{ status }}
    </p>
    <p
      data-testid="event-stream"
      class="text-xs text-slate-400"
    >
      {{ eventStream }}
    </p>
    <div class="flex items-center gap-6">
      <RouterLink
        to="/board"
        class="text-sm text-slate-500 underline-offset-4 hover:text-slate-900 hover:underline"
      >
        Global board
      </RouterLink>
      <RouterLink
        to="/register"
        class="text-sm text-slate-500 underline-offset-4 hover:text-slate-900 hover:underline"
      >
        Register a Project
      </RouterLink>
      <RouterLink
        to="/initiatives"
        class="text-sm text-slate-500 underline-offset-4 hover:text-slate-900 hover:underline"
      >
        Manage Initiatives
      </RouterLink>
      <RouterLink
        to="/settings/herdr"
        class="text-sm text-slate-500 underline-offset-4 hover:text-slate-900 hover:underline"
      >
        Herdr settings
      </RouterLink>
      <RouterLink
        to="/settings/profiles"
        class="text-sm text-slate-500 underline-offset-4 hover:text-slate-900 hover:underline"
      >
        Execution profiles
      </RouterLink>
      <RouterLink
        to="/settings/capacity"
        class="text-sm text-slate-500 underline-offset-4 hover:text-slate-900 hover:underline"
      >
        Capacity settings
      </RouterLink>
      <RouterLink
        to="/planning"
        class="text-sm text-slate-500 underline-offset-4 hover:text-slate-900 hover:underline"
      >
        Plan the Work
      </RouterLink>
      <RouterLink
        to="/planning/specs"
        class="text-sm text-slate-500 underline-offset-4 hover:text-slate-900 hover:underline"
      >
        Author Specs
      </RouterLink>
      <RouterLink
        to="/planning/tickets"
        class="text-sm text-slate-500 underline-offset-4 hover:text-slate-900 hover:underline"
      >
        Create Tickets
      </RouterLink>
      <RouterLink
        to="/planning/dependencies"
        class="text-sm text-slate-500 underline-offset-4 hover:text-slate-900 hover:underline"
      >
        Wire Dependencies
      </RouterLink>
    </div>
    <section
      v-if="connection.phase === 'connected'"
      class="mt-6 flex w-full max-w-2xl flex-col gap-6"
    >
      <label class="flex flex-col gap-1 text-sm text-slate-600">
        Project
        <select
          v-model="selectedProjectId"
          data-testid="home-project-select"
          aria-label="Project"
          class="rounded border border-slate-300 px-3 py-2 text-sm"
        >
          <option :value="null">
            Select a Project
          </option>
          <option
            v-for="entry in projects.projects"
            :key="entry.id"
            :value="entry.id"
          >
            {{ entry.code }} — {{ entry.name }}{{ entry.archived ? ' (archived)' : '' }}
          </option>
        </select>
      </label>

      <section
        v-if="selectedProjectId === null"
        data-testid="timeline-unselected"
        class="rounded-lg border border-slate-200 bg-white p-4 text-center shadow-sm"
      >
        <h2 class="text-lg font-semibold text-slate-900">
          Activity timeline
        </h2>
        <p class="mt-2 text-sm text-slate-500">
          Select a Project or entity to view its history.
        </p>
      </section>

      <template v-else-if="timelineScope">
        <RouterLink
          :to="`/projects/${selectedProjectId}/board`"
          data-testid="home-open-board"
          class="w-fit text-sm text-slate-500 underline-offset-4 hover:text-slate-900 hover:underline"
        >
          Open the {{ projects.projects.find((entry) => entry.id === selectedProjectId)?.code }} board
        </RouterLink>
        <TimelineSurface :scope="timelineScope" />
        <RulingsSurface :project-id="selectedProjectId" />
      </template>
    </section>
  </main>
</template>
