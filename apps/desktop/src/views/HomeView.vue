<script setup lang="ts">
// The boot surface: everything the operator sees before the board
// views land, bound to connection state from the generated client.
import { computed, inject, onMounted } from 'vue'
import { kanbanTransportKey } from '../core/transport'
import { useConnectionStore } from '../stores/connection'

const transport = inject(kanbanTransportKey)
const connection = useConnectionStore()

onMounted(() => {
  if (transport) {
    void connection.boot(transport)
  }
})

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
        to="/planning"
        class="text-sm text-slate-500 underline-offset-4 hover:text-slate-900 hover:underline"
      >
        Plan the Work
      </RouterLink>
    </div>
    <section
      v-if="connection.phase === 'connected'"
      data-testid="timeline-unselected"
      class="mt-6 w-full max-w-2xl rounded-lg border border-slate-200 bg-white p-4 text-center shadow-sm"
    >
      <h2 class="text-lg font-semibold text-slate-900">
        Activity timeline
      </h2>
      <p class="mt-2 text-sm text-slate-500">
        Select a Project or entity to view its history.
      </p>
    </section>
  </main>
</template>
