<script setup lang="ts">
// The boot surface: everything the operator sees before the board
// views land, bound to connection state from the generated client.
// Selecting a Project mounts its timeline and rulings under the
// numeric identity the core resolves (KAN-S2-US1, KAN-T79).
import { computed, inject, onMounted, ref, watch } from 'vue'
import { useRoute } from 'vue-router'
import RulingsSurface from '../components/RulingsSurface.vue'
import TimelineSurface from '../components/TimelineSurface.vue'
import { kanbanTransportKey } from '../core/transport'
import { adoptScope, emptyScope, projectScopeKey, scopeHolds } from '../core/scope-authority'
import { useConnectionStore } from '../stores/connection'
import { useProjectRegisterStore } from '../stores/project-register'

const transport = inject(kanbanTransportKey)
const route = useRoute()
const connection = useConnectionStore()
const projects = useProjectRegisterStore()
const selectedProjectId = ref<number | null>(null)

// The Project an Attention item's deferral — or any other link — named,
// so arriving here opens that Project's activity rather than nothing
// (KAN-T140-AC6).
const linkedProjectId = computed(() => {
  const raw = route.query.project
  const value = Array.isArray(raw) ? raw[0] : raw
  const parsed = Number(value)
  return value && Number.isInteger(parsed) && parsed > 0 ? parsed : null
})

// The observed role an Attention item is tracing, when one is named:
// the only record this application holds of a role is its telemetry
// on this Project's timeline (KAN-T140-AC6).
const linkedRole = computed(() => {
  const raw = route.query.role
  const value = Array.isArray(raw) ? raw[0] : raw
  return typeof value === 'string' && value.length > 0 ? value : null
})

const timelineScope = computed(() =>
  selectedProjectId.value === null ? null : { project: selectedProjectId.value },
)

// Every read carries the scope it was issued in, so a Project the
// link has left cannot select itself once its answer lands
// (KAN-T140-AC6, KAN-T145).
const scope = emptyScope()

onMounted(() => {
  if (transport) {
    void connection.boot(transport)
  }
})

// The activity on display follows the connection and the link
// together: this component is reused when a later link changes only
// the query, so a second Attention item must open its own Project's
// activity rather than leave the previous one mounted (KAN-T140-AC6).
watch(
  () => [connection.phase, linkedProjectId.value] as const,
  ([phase]) => {
    const claim = adoptScope(scope, projectScopeKey(linkedProjectId.value))
    // Synchronous: a link naming a Project takes the selection with
    // it before the register is read again, so no Project the route
    // left stays on display while that read is in flight.
    adoptLinkedProject()
    if (phase === 'connected' && transport) {
      void projects.refresh(transport).then(() => {
        if (!scopeHolds(scope, claim)) return
        adoptLinkedProject()
      })
    }
  },
  { immediate: true },
)

// The Project the link names, once the register holds it. A link
// naming none leaves the operator's own pick standing, because
// nothing then contradicts it.
function adoptLinkedProject(): void {
  const named = linkedProjectId.value
  if (named === null) return
  selectedProjectId.value = projects.projects.some((entry) => entry.id === named) ? named : null
}

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
  <main class="flex min-h-screen flex-col items-center justify-center gap-3 px-4 py-8">
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
    <div class="flex w-full max-w-2xl flex-wrap items-center justify-center gap-x-6 gap-y-2 px-4">
      <RouterLink
        to="/attention"
        data-testid="attention-link"
        class="text-sm text-slate-500 underline-offset-4 hover:text-slate-900 hover:underline"
      >
        Attention Inbox
      </RouterLink>
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
        to="/health"
        class="text-sm text-slate-500 underline-offset-4 hover:text-slate-900 hover:underline"
      >
        Component health
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
        to="/planning/dependencies"
        class="text-sm text-slate-500 underline-offset-4 hover:text-slate-900 hover:underline"
      >
        Wire Dependencies
      </RouterLink>
    </div>
    <section
      v-if="connection.phase === 'connected'"
      class="mt-6 flex w-full max-w-2xl min-w-0 flex-col gap-6"
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
        <p
          v-if="linkedRole"
          data-testid="activity-role"
          class="text-sm text-slate-600"
        >
          Tracing the observed role {{ linkedRole }}: its reported activity is marked below.
        </p>
        <TimelineSurface
          :scope="timelineScope"
          :role="linkedRole"
        />
        <RulingsSurface :project-id="selectedProjectId" />
      </template>
    </section>
  </main>
</template>
