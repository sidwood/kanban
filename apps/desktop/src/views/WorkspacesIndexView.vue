<script setup lang="ts">
// The Workspaces & Lanes front door when no Project is in scope: one
// row per registered Project, each opening that Project's Workspace
// surface. Presentation only; the register store answers.
import { inject, onMounted } from 'vue'
import EmptyState from '../components/EmptyState.vue'
import InlineAlert from '../components/InlineAlert.vue'
import SectionHeader from '../components/SectionHeader.vue'
import { kanbanTransportKey } from '../core/transport'
import { useProjectRegisterStore } from '../stores/project-register'

const transport = inject(kanbanTransportKey)
const projects = useProjectRegisterStore()

onMounted(() => {
  if (transport) void projects.refresh(transport)
})
</script>

<template>
  <main class="animate-rise flex flex-col gap-6 px-6 py-8 lg:px-8">
    <SectionHeader
      eyebrow="Execution"
      title="Workspaces & Lanes"
      summary="Each Project observes its own Workspaces and Lanes. Choose a Project, or scope the board to one and this surface follows."
    />

    <InlineAlert v-if="projects.error">
      {{ projects.error }}
    </InlineAlert>

    <p
      v-else-if="!projects.loaded"
      class="text-sm text-ink-subtle"
    >
      Loading Projects…
    </p>

    <div
      v-else-if="projects.projects.length === 0"
      class="flex flex-col gap-3"
    >
      <EmptyState
        message="No Project is registered yet."
        hint="Register a Project to observe its Workspaces and Lanes."
      />
      <RouterLink
        to="/register"
        data-testid="workspaces-register"
        class="w-fit text-sm text-accent underline-offset-4 hover:underline"
      >
        Register a Project
      </RouterLink>
    </div>

    <ul
      v-else
      class="flex flex-col divide-y divide-line overflow-hidden rounded-panel border border-line bg-surface"
      data-testid="workspaces-projects"
    >
      <li
        v-for="project in projects.projects"
        :key="project.id"
      >
        <RouterLink
          :to="`/projects/${project.id}/workspaces`"
          :data-testid="`workspaces-project-${project.id}`"
          class="flex items-center gap-4 px-4 py-3 transition-colors hover:bg-accent/6"
        >
          <span class="w-16 shrink-0 font-mono text-xs font-semibold text-accent">
            {{ project.code }}
          </span>
          <span class="flex min-w-0 flex-1 flex-col">
            <span class="text-sm font-medium text-ink">{{ project.name }}</span>
            <span class="truncate text-xs text-ink-subtle">
              {{ project.repository }} · {{ project.default_branch }}
            </span>
          </span>
          <span
            v-if="project.archived"
            class="text-[0.625rem] font-semibold tracking-wide text-ink-subtle uppercase"
          >
            Archived
          </span>
        </RouterLink>
      </li>
    </ul>
  </main>
</template>
