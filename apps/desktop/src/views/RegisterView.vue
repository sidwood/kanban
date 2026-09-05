<script setup lang="ts">
// The Project registration surface: register a Project with its
// one target repository, Seed Workspace, default branch, and
// exclusive Herdr session, optionally under one Initiative, then
// list and archive through the project-register store. Presentation
// only; every domain call goes through the generated client in the
// store, and no delete control exists (KAN-S1-US4, KAN-S1-US6).
import { computed, inject, onMounted, reactive } from 'vue'
import { kanbanTransportKey } from '../core/transport'
import { useInitiativesStore } from '../stores/initiatives'
import { useProjectRegisterStore } from '../stores/project-register'

const transport = inject(kanbanTransportKey)
const projects = useProjectRegisterStore()
const initiatives = useInitiativesStore()
const draft = reactive({
  code: '',
  name: '',
  repository: '',
  seed_workspace: '',
  default_branch: '',
  herdr_session: '',
  initiative_id: null as number | null,
})

onMounted(() => {
  if (transport) {
    void projects.refresh(transport)
    void initiatives.refresh(transport)
  }
})

// The picker lists every Initiative: an optional group is a group,
// archived or not.
const initiativeOptions = computed(() => initiatives.initiatives)

const draftCarriesEveryAnchor = computed(() =>
  [
    draft.code,
    draft.name,
    draft.repository,
    draft.seed_workspace,
    draft.default_branch,
    draft.herdr_session,
  ].every((field) => field.trim().length > 0),
)

async function submitRegister() {
  if (!transport || !draftCarriesEveryAnchor.value) {
    return
  }
  await projects.register(transport, { ...draft })
  if (!projects.error) {
    draft.code = ''
    draft.name = ''
    draft.repository = ''
    draft.seed_workspace = ''
    draft.default_branch = ''
    draft.herdr_session = ''
    draft.initiative_id = null
  }
}

async function submitArchive(id: number) {
  if (transport) {
    await projects.archive(transport, id)
  }
}
</script>

<template>
  <main class="mx-auto flex min-h-screen max-w-3xl flex-col gap-6 p-8">
    <nav class="text-sm text-slate-500">
      <RouterLink
        to="/"
        class="hover:text-slate-900"
      >
        Kanban
      </RouterLink>
      <span aria-hidden="true"> / </span>
      <span class="text-slate-900">Register</span>
    </nav>

    <h1 class="text-3xl font-semibold tracking-tight">
      Register a Project
    </h1>

    <p
      v-if="projects.error"
      data-testid="project-error"
      role="alert"
      class="rounded border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-700"
    >
      {{ projects.error }}
    </p>

    <form
      class="flex flex-col gap-3"
      @submit.prevent="submitRegister"
    >
      <div class="flex flex-wrap gap-3">
        <input
          v-model="draft.code"
          data-testid="project-code"
          aria-label="Project code"
          placeholder="Code, for example CORE"
          class="w-44 rounded border border-slate-300 px-3 py-2 text-sm"
        >
        <input
          v-model="draft.name"
          data-testid="project-name"
          aria-label="Project name"
          placeholder="Project name"
          class="flex-1 rounded border border-slate-300 px-3 py-2 text-sm"
        >
        <select
          v-model="draft.initiative_id"
          data-testid="project-initiative"
          aria-label="Initiative"
          class="rounded border border-slate-300 px-3 py-2 text-sm"
        >
          <option :value="null">
            No Initiative
          </option>
          <option
            v-for="initiative in initiativeOptions"
            :key="initiative.id"
            :value="initiative.id"
          >
            {{ initiative.name }}
          </option>
        </select>
      </div>
      <input
        v-model="draft.repository"
        data-testid="project-repository"
        aria-label="Target Git repository"
        placeholder="Target Git repository"
        class="rounded border border-slate-300 px-3 py-2 text-sm"
      >
      <div class="flex flex-wrap gap-3">
        <input
          v-model="draft.seed_workspace"
          data-testid="project-seed"
          aria-label="Seed Workspace"
          placeholder="Seed Workspace"
          class="flex-1 rounded border border-slate-300 px-3 py-2 text-sm"
        >
        <input
          v-model="draft.default_branch"
          data-testid="project-branch"
          aria-label="Default branch"
          placeholder="Default branch"
          class="w-48 rounded border border-slate-300 px-3 py-2 text-sm"
        >
        <input
          v-model="draft.herdr_session"
          data-testid="project-session"
          aria-label="Herdr session name"
          placeholder="Herdr session name"
          class="w-56 rounded border border-slate-300 px-3 py-2 text-sm"
        >
      </div>
      <button
        type="submit"
        data-testid="project-register"
        class="w-fit rounded bg-slate-900 px-3 py-2 text-sm font-medium text-white hover:bg-slate-700"
      >
        Register
      </button>
    </form>

    <ul
      v-if="projects.loaded"
      data-testid="project-list"
      class="flex flex-col divide-y divide-slate-200 rounded border border-slate-200"
    >
      <li
        v-for="project in projects.projects"
        :key="project.id"
        :data-testid="`project-row-${project.id}`"
        class="flex flex-wrap items-center gap-3 px-4 py-3"
      >
        <span
          :data-testid="`project-code-${project.id}`"
          class="rounded bg-slate-100 px-2 py-0.5 font-mono text-sm font-medium"
        >
          {{ project.code }}
        </span>
        <span
          :data-testid="`project-name-${project.id}`"
          class="font-medium"
        >
          {{ project.name }}
        </span>
        <span class="text-xs text-slate-500">
          {{ project.repository }} · {{ project.default_branch }} ·
          {{ project.herdr_session }}
        </span>
        <span
          :data-testid="`project-counters-${project.id}`"
          class="font-mono text-xs text-slate-500"
        >
          P{{ project.counters.plan }} · S{{ project.counters.spec }} ·
          T{{ project.counters.ticket }}
        </span>
        <span
          v-if="project.archived"
          :data-testid="`project-archived-${project.id}`"
          class="rounded bg-slate-100 px-2 py-0.5 text-xs text-slate-600"
        >
          Archived
        </span>
        <button
          v-else
          :data-testid="`project-archive-${project.id}`"
          class="rounded border border-slate-300 px-2 py-1 text-sm hover:bg-slate-50"
          @click="submitArchive(project.id)"
        >
          Archive
        </button>
        <RouterLink
          :to="`/projects/${project.id}/workspaces`"
          :data-testid="`project-workspaces-${project.id}`"
          class="rounded border border-slate-300 px-2 py-1 text-sm hover:bg-slate-50"
        >
          Workspaces
        </RouterLink>
      </li>
    </ul>
    <p
      v-else-if="!projects.error"
      data-testid="project-loading"
    >
      Loading Projects…
    </p>
  </main>
</template>
