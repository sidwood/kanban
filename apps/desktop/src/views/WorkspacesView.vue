<script setup lang="ts">
// Workspaces & Lanes for one Project, composed into the shell
// (KAN-T140-AC3): the Workspaces the core registered and observed with
// the health it decided (KAN-S6-US1), the Lanes that claim them under
// the assignment constraints, the Ticket each Lane holds, the Run
// occupying it now and every attempt behind that Run (KAN-S6-US2), and
// one read-only summary of the capacity that bounds the Project — the
// only capacity surface this view carries (KAN-S7-US3). Presentation
// only; health, reuse, and fallback truth are the core's, never
// recomputed here.
import { computed, inject, reactive, ref, watch } from 'vue'
import { useRoute } from 'vue-router'
import { KanbanClient } from '@kanban/contracts'
import type {
  CapacityGlobalDefaults,
  CapacityProjectCaps,
  RunRecord,
  TicketRecord,
} from '@kanban/contracts'
import { asApiError, kanbanTransportKey } from '../core/transport'
import {
  adoptScope,
  emptyScope,
  projectScopeKey,
  scopeHolds,
} from '../core/scope-authority'
import type { ScopeClaim } from '../core/scope-authority'
import { useLanesStore } from '../stores/lanes'
import { useProjectRegisterStore } from '../stores/project-register'
import { useWorkspacesStore } from '../stores/workspaces'
import AppButton from '../components/AppButton.vue'
import InlineAlert from '../components/InlineAlert.vue'
import SectionHeader from '../components/SectionHeader.vue'
import StatusBadge from '../components/StatusBadge.vue'
import type { StatusTone } from '../components/StatusBadge.vue'

const transport = inject(kanbanTransportKey)
const route = useRoute()
const projects = useProjectRegisterStore()
const workspaces = useWorkspacesStore()
const lanes = useLanesStore()
const draft = reactive({ path: '' })
const laneChoice = reactive<Record<number, string>>({})

// The execution the Project is actually carrying.
const runs = ref<RunRecord[]>([])
const tickets = ref<TicketRecord[]>([])
const defaults = ref<CapacityGlobalDefaults | null>(null)
const caps = ref<CapacityProjectCaps | null>(null)
const executionError = ref<string | null>(null)

const projectId = computed(() => Number(route.params.projectId))

const project = computed(() =>
  projects.projects.find((entry) => entry.id === projectId.value) ?? null,
)

/** The Run one link names, when it names one. */
const linkedRunId = computed(() => {
  const raw = route.query.run
  const value = Array.isArray(raw) ? raw[0] : raw
  const parsed = Number(value)
  return value && Number.isInteger(parsed) && parsed > 0 ? parsed : null
})

// Every load carries the scope it was issued in. Vue Router reuses
// this component when a link changes only the Project parameter, so a
// load for the Project the operator has left must never write over
// the one they are in (KAN-T140-AC3, KAN-T145).
const scope = emptyScope()

// Follow the exact Project the route names, at mount and every time a
// link changes it on this same component.
watch(
  projectId,
  () => {
    void adoptProject()
  },
  { immediate: true },
)

async function adoptProject(): Promise<void> {
  if (!transport) {
    return
  }
  const target = projectId.value
  const claim = adoptScope(scope, projectScopeKey(target))
  forgetProjectScope()
  await projects.refresh(transport)
  if (!scopeHolds(scope, claim) || !project.value) {
    return
  }
  await Promise.all([
    workspaces.load(transport, target),
    lanes.load(transport, target),
    loadExecution(claim, target),
  ])
}

// Drop everything the Project being left owns: its Workspaces, Lanes,
// execution, caps, and the drafts in this surface's own controls.
function forgetProjectScope(): void {
  draft.path = ''
  for (const key of Object.keys(laneChoice)) {
    delete laneChoice[Number(key)]
  }
  runs.value = []
  tickets.value = []
  defaults.value = null
  caps.value = null
  executionError.value = null
  workspaces.clear()
  lanes.clear()
}

// The Runs, the Tickets they execute, and the caps that bound them.
// An answer for a Project the surface has left writes nothing.
async function loadExecution(claim: ScopeClaim, target: number): Promise<void> {
  if (!transport) {
    return
  }
  const client = new KanbanClient(transport)
  try {
    const [runList, ticketList, globals, project] = await Promise.all([
      client.queryRunList({ project_id: target }),
      client.queryTicketList({ project_id: target }),
      client.queryCapacityDefaultsGet({}),
      client.queryCapacitySettingsGet({ project_id: target }),
    ])
    if (!scopeHolds(scope, claim)) {
      return
    }
    runs.value = runList.runs
    tickets.value = ticketList.tickets
    defaults.value = globals.defaults
    caps.value = project.caps
    executionError.value = null
  } catch (failure) {
    if (!scopeHolds(scope, claim)) {
      return
    }
    executionError.value = asApiError(failure).message
  }
}

const draftCarriesPath = computed(() => draft.path.trim().length > 0)

const claimableLanes = computed(() => lanes.lanes)

function laneWorkspacePath(laneId: number): string | null {
  const lane = lanes.lanes.find((entry) => entry.id === laneId)
  if (!lane?.workspace_id) {
    return null
  }
  return workspaces.workspaces.find((entry) => entry.id === lane.workspace_id)?.path ?? null
}

// What one Lane is carrying: the identity of the Ticket it holds, the
// Run occupying it now — the executing one, the only status that holds
// an execution open — and every Run of that Ticket newest first.
interface LaneExecution {
  ticket: string
  current: RunRecord | null
  attempts: RunRecord[]
}

const laneExecution = computed<Record<number, LaneExecution>>(() =>
  Object.fromEntries(
    lanes.lanes
      .filter((lane) => lane.ticket_id !== null)
      .map((lane) => {
        const ticketId = lane.ticket_id as number
        const held = tickets.value.find((entry) => entry.id === ticketId)
        const attempts = runs.value
          .filter((entry) => entry.ticket_id === ticketId)
          .sort((left, right) => right.id - left.id)
        return [
          lane.id,
          {
            // Until the Ticket rows are read, a Lane can only name the
            // identity it holds itself.
            ticket: held ? `${project.value?.code ?? ''}-T${held.number}` : `Ticket ${ticketId}`,
            current: attempts.find((entry) => entry.status === 'executing') ?? null,
            attempts,
          },
        ]
      }),
  ),
)

const workspaceTones: Record<string, StatusTone> = {
  available: 'positive',
  assigned: 'progress',
  dirty: 'caution',
  missing: 'critical',
  retired: 'neutral',
  unobserved: 'caution',
}

// The Lanes holding a Ticket now, against the Project's Lane cap.
const activeLanes = computed(() => lanes.lanes.filter((lane) => lane.ticket_id !== null).length)

// One capacity line: the Project's stricter ceiling when it imposes
// one, else the global default, named for what it is (DR-EP-06,
// DR-EP-07).
function ceiling(field: 'max_active_per_harness' | 'max_active_per_model' | 'max_active_per_usage_pool') {
  const project = caps.value?.[field] ?? null
  if (project !== null) {
    return { limit: project, origin: 'Project cap' }
  }
  const global = defaults.value?.[field] ?? null
  return global === null ? null : { limit: global, origin: 'global default' }
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
  <main class="animate-rise flex flex-col gap-6 px-6 py-8 lg:px-8">
    <SectionHeader
      eyebrow="Execution"
      title="Workspaces & Lanes"
      :summary="project
        ? `Where ${project.code} runs: the working copies the core observes, the Lanes that claim them, and what each Lane is executing now.`
        : undefined"
    />

    <InlineAlert
      v-if="!project"
      data-testid="workspace-project-missing"
    >
      Project {{ projectId }} is not registered.
    </InlineAlert>

    <InlineAlert
      v-if="workspaces.error"
      data-testid="workspace-error"
    >
      {{ workspaces.error }}
    </InlineAlert>

    <InlineAlert
      v-if="executionError"
      data-testid="execution-error"
    >
      {{ executionError }}
    </InlineAlert>

    <section
      v-if="project"
      data-testid="capacity-summary"
      class="flex flex-wrap gap-6 rounded-panel border border-line bg-surface p-4"
    >
      <div class="flex flex-col gap-1">
        <p class="text-[0.6rem] font-bold tracking-[0.13em] text-ink-subtle uppercase">
          Capacity
        </p>
        <p class="text-xs text-ink-subtle">
          Settings live in Capacity settings; this is the standing summary.
        </p>
      </div>
      <dl class="flex flex-wrap gap-6">
        <div
          data-testid="capacity-lanes"
          class="flex flex-col gap-0.5"
        >
          <dt class="text-xs text-ink-subtle">
            Active Lanes
          </dt>
          <dd class="font-mono text-sm text-ink">
            {{ caps?.max_active_lanes == null
              ? `${activeLanes} of no cap`
              : `${activeLanes} of ${caps.max_active_lanes}` }}
          </dd>
        </div>
        <div
          data-testid="capacity-harness"
          class="flex flex-col gap-0.5"
        >
          <dt class="text-xs text-ink-subtle">
            Active runs per harness
          </dt>
          <dd class="font-mono text-sm text-ink">
            {{ ceiling('max_active_per_harness')
              ? `${ceiling('max_active_per_harness')!.limit} · ${ceiling('max_active_per_harness')!.origin}`
              : 'no cap' }}
          </dd>
        </div>
        <div
          data-testid="capacity-model"
          class="flex flex-col gap-0.5"
        >
          <dt class="text-xs text-ink-subtle">
            Active runs per model family
          </dt>
          <dd class="font-mono text-sm text-ink">
            {{ ceiling('max_active_per_model')
              ? `${ceiling('max_active_per_model')!.limit} · ${ceiling('max_active_per_model')!.origin}`
              : 'no cap' }}
          </dd>
        </div>
        <div
          data-testid="capacity-pool"
          class="flex flex-col gap-0.5"
        >
          <dt class="text-xs text-ink-subtle">
            Active runs per usage pool
          </dt>
          <dd class="font-mono text-sm text-ink">
            {{ ceiling('max_active_per_usage_pool')
              ? `${ceiling('max_active_per_usage_pool')!.limit} · ${ceiling('max_active_per_usage_pool')!.origin}`
              : 'no cap' }}
          </dd>
        </div>
      </dl>
    </section>

    <form
      v-if="project"
      class="flex flex-wrap items-end gap-3"
      @submit.prevent="submitRegister"
    >
      <label class="flex min-w-72 flex-1 flex-col gap-1 text-sm text-ink-muted">
        Workspace path
        <input
          v-model="draft.path"
          data-testid="workspace-path"
          aria-label="Workspace path"
          placeholder="/workspaces/kanban.feature"
          class="rounded-control border border-line bg-surface px-3 py-2 text-sm text-ink"
        >
      </label>
      <AppButton
        type="submit"
        data-testid="workspace-register"
        variant="primary"
        size="sm"
      >
        Register Workspace
      </AppButton>
    </form>

    <ul
      v-if="workspaces.loaded"
      data-testid="workspace-list"
      class="flex flex-col divide-y divide-line overflow-hidden rounded-panel border border-line bg-surface"
    >
      <li
        v-for="workspace in workspaces.workspaces"
        :key="workspace.id"
        :data-testid="`workspace-row-${workspace.id}`"
        class="flex flex-wrap items-center gap-3 px-4 py-3"
      >
        <StatusBadge
          :data-testid="`workspace-health-${workspace.id}`"
          :tone="workspaceTones[workspace.health] ?? 'neutral'"
          density="compact"
        >
          {{ workspace.health }}
        </StatusBadge>
        <span
          v-if="workspace.health === 'unobserved'"
          :data-testid="`workspace-unobserved-${workspace.id}`"
          title="git status could not be read; the tree state is unknown"
          class="rounded-control bg-caution-soft px-2 py-0.5 text-xs text-caution"
        >
          observation failed
        </span>
        <span
          :data-testid="`workspace-path-${workspace.id}`"
          class="max-w-full break-all font-mono text-sm text-ink"
        >
          {{ workspace.path }}
        </span>
        <span
          v-if="workspace.is_seed"
          :data-testid="`workspace-seed-${workspace.id}`"
          class="rounded-control bg-caution-soft px-2 py-0.5 text-xs text-caution"
        >
          Seed
        </span>
        <span
          v-if="workspace.observation.checkout === 'detached'"
          :data-testid="`workspace-detached-${workspace.id}`"
          class="rounded-control bg-rail px-2 py-0.5 font-mono text-xs text-ink-muted"
        >
          detached
        </span>
        <span
          v-else-if="workspace.observation.branch"
          class="text-xs text-ink-subtle"
        >
          {{ workspace.observation.branch }}
        </span>
        <span
          v-if="workspace.observation.lane_assignment !== null"
          :data-testid="`workspace-lane-${workspace.id}`"
          class="rounded-control bg-info/12 px-2 py-0.5 text-xs text-info"
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
            class="min-w-0 max-w-full rounded-control border border-line bg-surface px-2 py-1 text-sm text-ink"
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
          <AppButton
            type="submit"
            :data-testid="`workspace-lane-assign-${workspace.id}`"
            size="sm"
          >
            Assign
          </AppButton>
        </form>
        <AppButton
          :data-testid="`workspace-observe-${workspace.id}`"
          size="sm"
          @click="submitObserve(workspace.id)"
        >
          Observe
        </AppButton>
      </li>
    </ul>
    <p
      v-else-if="project && !workspaces.error"
      data-testid="workspace-loading"
      class="text-sm text-ink-subtle"
    >
      Loading Workspaces…
    </p>

    <section
      v-if="project"
      class="flex flex-col gap-3"
    >
      <div class="flex flex-wrap items-center justify-between gap-3">
        <h2 class="font-display text-xl font-semibold tracking-tight text-ink">
          Lanes
        </h2>
        <form @submit.prevent="submitCreateLane">
          <AppButton
            type="submit"
            data-testid="lane-create"
            variant="primary"
            size="sm"
          >
            Create Lane
          </AppButton>
        </form>
      </div>
      <InlineAlert
        v-if="lanes.error"
        data-testid="lane-error"
      >
        {{ lanes.error }}
      </InlineAlert>
      <ul
        v-if="lanes.loaded"
        data-testid="lane-list"
        class="flex flex-col divide-y divide-line overflow-hidden rounded-panel border border-line bg-surface"
      >
        <li
          v-for="lane in lanes.lanes"
          :key="lane.id"
          :data-testid="`lane-row-${lane.id}`"
          class="flex flex-col gap-2 px-4 py-3"
        >
          <div class="flex flex-wrap items-center gap-3">
            <span
              :data-testid="`lane-id-${lane.id}`"
              class="rounded-control bg-info/12 px-2 py-0.5 font-mono text-xs text-info uppercase"
            >
              Lane {{ lane.id }}
            </span>
            <span
              :data-testid="`lane-workspace-${lane.id}`"
              class="max-w-full break-all font-mono text-sm text-ink"
            >
              {{ laneWorkspacePath(lane.id) ?? 'no Workspace claimed' }}
            </span>
            <span
              v-if="laneExecution[lane.id]"
              :data-testid="`lane-ticket-${lane.id}`"
              class="font-mono text-xs text-ink-muted"
            >
              {{ laneExecution[lane.id]!.ticket }}
            </span>
            <form
              v-if="lane.workspace_id !== null"
              class="ml-auto"
              @submit.prevent="submitReleaseLane(lane.id)"
            >
              <AppButton
                type="submit"
                :data-testid="`lane-release-${lane.id}`"
                size="sm"
              >
                Release
              </AppButton>
            </form>
          </div>

          <template v-if="laneExecution[lane.id]">
            <div
              v-if="laneExecution[lane.id]!.current"
              :data-testid="`lane-run-${lane.id}`"
              class="flex flex-col gap-1 rounded-control border border-line bg-rail px-3 py-2"
            >
              <p class="text-xs text-ink-muted">
                <span class="font-mono text-ink">Run {{ laneExecution[lane.id]!.current!.id }}</span>
                · {{ laneExecution[lane.id]!.current!.status }}
                · requested {{ laneExecution[lane.id]!.current!.requested.name }}
                · effective {{ laneExecution[lane.id]!.current!.effective.name }}
              </p>
              <p
                v-if="laneExecution[lane.id]!.current!.fallback"
                :data-testid="`lane-run-fallback-${lane.id}`"
                class="text-xs text-caution"
              >
                Fallback: {{ laneExecution[lane.id]!.current!.fallback_path.join(' → ') }}
              </p>
            </div>
            <p
              v-else
              :data-testid="`lane-no-run-${lane.id}`"
              class="text-xs text-ink-subtle"
            >
              No Run occupies this Lane.
            </p>

            <ul
              v-if="laneExecution[lane.id]!.attempts.length"
              :data-testid="`lane-attempts-${lane.id}`"
              class="flex flex-col gap-1"
            >
              <li
                v-for="attempt in laneExecution[lane.id]!.attempts"
                :key="attempt.id"
                :data-testid="`lane-attempt-${attempt.id}`"
                :data-linked="attempt.id === linkedRunId ? 'true' : undefined"
                class="flex flex-wrap items-center gap-2 rounded-control px-2 py-1 text-xs"
                :class="attempt.id === linkedRunId ? 'bg-accent/10 text-ink' : 'text-ink-subtle'"
              >
                <span class="font-mono">Run {{ attempt.id }}</span>
                <span>{{ attempt.status }}</span>
                <span>{{ attempt.effective.name }}</span>
                <span v-if="attempt.fallback">(fallback from {{ attempt.requested.name }})</span>
              </li>
            </ul>
          </template>
        </li>
      </ul>
      <p
        v-else-if="project && !lanes.error"
        data-testid="lane-loading"
        class="text-sm text-ink-subtle"
      >
        Loading Lanes…
      </p>
    </section>
  </main>
</template>
