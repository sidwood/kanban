<script setup lang="ts">
// Workspace management for one Project: register paths, observe git
// state, and surface health (KAN-S6-US1); Lanes claim Workspaces and
// hold Tickets under the assignment constraints (KAN-S6-US2).
import { computed, inject, onMounted, reactive } from 'vue'
import { useRoute } from 'vue-router'
import { kanbanTransportKey } from '../core/transport'
import { useLanesStore } from '../stores/lanes'
import { useProjectRegisterStore } from '../stores/project-register'
import { useWorkspacesStore } from '../stores/workspaces'

const transport = inject(kanbanTransportKey)
const route = useRoute()
const projects = useProjectRegisterStore()
const workspaces = useWorkspacesStore()
const lanes = useLanesStore()
const draft = reactive({ path: '' })
const laneChoice = reactive<Record<number, string>>({})

const projectId = computed(() => Number(route.params.projectId))

const project = computed(() =>
  projects.projects.find((entry) => entry.id === projectId.value) ?? null,
)

onMounted(async () => {
  if (!transport) {
    return
  }
  await projects.refresh(transport)
  if (project.value) {
    await Promise.all([
      workspaces.load(transport, projectId.value),
      lanes.load(transport, projectId.value),
    ])
  }
})

const draftCarriesPath = computed(() => draft.path.trim().length > 0)

const claimableLanes = computed(() => lanes.lanes)

function laneWorkspacePath(laneId: number): string | null {
  const lane = lanes.lanes.find((entry) => entry.id === laneId)
  if (!lane?.workspace_id) {
    return null
  }
  return workspaces.workspaces.find((entry) => entry.id === lane.workspace_id)?.path ?? null
}

async function submitRegister() {
  if (!transport || !draftCarriesPath.value) {
    return
  }
  await workspaces.register(transport, projectId.value, { path: draft.path })
  if (!workspaces.error) {
    draft.path = ''
  }
}

async function submitObserve(id: number) {
  if (transport) {
    await workspaces.observe(transport, projectId.value, id)
  }
}

async function submitCreateLane() {
  if (transport) {
    await lanes.create(transport, projectId.value)
  }
}

async function submitAssignLane(workspaceId: number) {
  if (!transport) {
    return
  }
  const laneId = Number(laneChoice[workspaceId])
  if (!laneId) {
    return
  }
  await lanes.assignWorkspace(transport, projectId.value, laneId, workspaceId)
  await workspaces.load(transport, projectId.value)
}

async function submitReleaseLane(laneId: number) {
  if (transport) {
    await lanes.releaseWorkspace(transport, projectId.value, laneId)
    await workspaces.load(transport, projectId.value)
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
      <RouterLink
        to="/register"
        class="hover:text-slate-900"
      >
        Register
      </RouterLink>
      <span aria-hidden="true"> / </span>
      <span class="text-slate-900">Workspaces</span>
    </nav>

    <h1
      v-if="project"
      class="text-3xl font-semibold tracking-tight"
    >
      Workspaces for {{ project.code }}
    </h1>
    <p
      v-else
      data-testid="workspace-project-missing"
      class="text-sm text-red-700"
    >
      Project {{ projectId }} is not registered.
    </p>

    <p
      v-if="workspaces.error"
      data-testid="workspace-error"
      role="alert"
      class="rounded border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-700"
    >
      {{ workspaces.error }}
    </p>

    <form
      v-if="project"
      class="flex flex-col gap-3"
      @submit.prevent="submitRegister"
    >
      <input
        v-model="draft.path"
        data-testid="workspace-path"
        aria-label="Workspace path"
        placeholder="Workspace path"
        class="rounded border border-slate-300 px-3 py-2 text-sm"
      >
      <button
        type="submit"
        data-testid="workspace-register"
        class="w-fit rounded bg-slate-900 px-3 py-2 text-sm font-medium text-white hover:bg-slate-700"
      >
        Register Workspace
      </button>
    </form>

    <ul
      v-if="workspaces.loaded"
      data-testid="workspace-list"
      class="flex flex-col divide-y divide-slate-200 rounded border border-slate-200"
    >
      <li
        v-for="workspace in workspaces.workspaces"
        :key="workspace.id"
        :data-testid="`workspace-row-${workspace.id}`"
        class="flex flex-wrap items-center gap-3 px-4 py-3"
      >
        <span
          :data-testid="`workspace-health-${workspace.id}`"
          class="rounded bg-slate-100 px-2 py-0.5 font-mono text-xs uppercase"
        >
          {{ workspace.health }}
        </span>
        <span
          v-if="workspace.health === 'unobserved'"
          :data-testid="`workspace-unobserved-${workspace.id}`"
          title="git status could not be read; the tree state is unknown"
          class="rounded bg-amber-50 px-2 py-0.5 text-xs text-amber-800"
        >
          observation failed
        </span>
        <span
          :data-testid="`workspace-path-${workspace.id}`"
          class="font-mono text-sm"
        >
          {{ workspace.path }}
        </span>
        <span
          v-if="workspace.is_seed"
          :data-testid="`workspace-seed-${workspace.id}`"
          class="rounded bg-amber-50 px-2 py-0.5 text-xs text-amber-800"
        >
          Seed
        </span>
        <span
          v-if="workspace.observation.checkout === 'detached'"
          :data-testid="`workspace-detached-${workspace.id}`"
          class="rounded bg-slate-100 px-2 py-0.5 font-mono text-xs text-slate-600"
        >
          detached
        </span>
        <span
          v-else-if="workspace.observation.branch"
          class="text-xs text-slate-500"
        >
          {{ workspace.observation.branch }}
        </span>
        <span
          v-if="workspace.observation.lane_assignment !== null"
          :data-testid="`workspace-lane-${workspace.id}`"
          class="rounded bg-indigo-50 px-2 py-0.5 text-xs text-indigo-800"
        >
          Lane {{ workspace.observation.lane_assignment }}
        </span>
        <form
          class="flex items-center gap-1"
          @submit.prevent="submitAssignLane(workspace.id)"
        >
          <select
            v-model="laneChoice[workspace.id]"
            :data-testid="`workspace-lane-select-${workspace.id}`"
            :aria-label="`Lane for ${workspace.path}`"
            class="rounded border border-slate-300 px-2 py-1 text-sm"
          >
            <option value="">
              Lane…
            </option>
            <option
              v-for="lane in claimableLanes"
              :key="lane.id"
              :value="String(lane.id)"
            >
              Lane {{ lane.id }}
            </option>
          </select>
          <button
            type="submit"
            :data-testid="`workspace-lane-assign-${workspace.id}`"
            class="rounded border border-slate-300 px-2 py-1 text-sm hover:bg-slate-50"
          >
            Assign
          </button>
        </form>
        <button
          :data-testid="`workspace-observe-${workspace.id}`"
          class="rounded border border-slate-300 px-2 py-1 text-sm hover:bg-slate-50"
          @click="submitObserve(workspace.id)"
        >
          Observe
        </button>
      </li>
    </ul>
    <p
      v-else-if="project && !workspaces.error"
      data-testid="workspace-loading"
    >
      Loading Workspaces…
    </p>

    <section
      v-if="project"
      class="flex flex-col gap-3"
    >
      <h2 class="text-xl font-semibold tracking-tight">
        Lanes
      </h2>
      <p
        v-if="lanes.error"
        data-testid="lane-error"
        role="alert"
        class="rounded border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-700"
      >
        {{ lanes.error }}
      </p>
      <form @submit.prevent="submitCreateLane">
        <button
          type="submit"
          data-testid="lane-create"
          class="w-fit rounded bg-slate-900 px-3 py-2 text-sm font-medium text-white hover:bg-slate-700"
        >
          Create Lane
        </button>
      </form>
      <ul
        v-if="lanes.loaded"
        data-testid="lane-list"
        class="flex flex-col divide-y divide-slate-200 rounded border border-slate-200"
      >
        <li
          v-for="lane in lanes.lanes"
          :key="lane.id"
          :data-testid="`lane-row-${lane.id}`"
          class="flex flex-wrap items-center gap-3 px-4 py-3"
        >
          <span
            :data-testid="`lane-id-${lane.id}`"
            class="rounded bg-indigo-50 px-2 py-0.5 font-mono text-xs uppercase text-indigo-800"
          >
            Lane {{ lane.id }}
          </span>
          <span
            :data-testid="`lane-workspace-${lane.id}`"
            class="font-mono text-sm"
          >
            {{ laneWorkspacePath(lane.id) ?? 'no Workspace claimed' }}
          </span>
          <span
            v-if="lane.ticket_id !== null"
            :data-testid="`lane-ticket-${lane.id}`"
            class="text-xs text-slate-500"
          >
            Ticket {{ lane.ticket_id }}
          </span>
          <form
            v-if="lane.workspace_id !== null"
            @submit.prevent="submitReleaseLane(lane.id)"
          >
            <button
              type="submit"
              :data-testid="`lane-release-${lane.id}`"
              class="rounded border border-slate-300 px-2 py-1 text-sm hover:bg-slate-50"
            >
              Release
            </button>
          </form>
        </li>
      </ul>
      <p
        v-else-if="project && !lanes.error"
        data-testid="lane-loading"
      >
        Loading Lanes…
      </p>
    </section>
  </main>
</template>
