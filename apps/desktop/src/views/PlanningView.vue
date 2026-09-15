<script setup lang="ts">
import { computed, inject, ref, watch } from 'vue'
import { useRoute } from 'vue-router'
import type { TicketRecord } from '@kanban/contracts'
import { KanbanClient } from '@kanban/contracts'
import { asApiError, kanbanTransportKey } from '../core/transport'
import {
  adoptScope,
  emptyScope,
  issueCommand,
  releaseScope,
  scopeHolds,
} from '../core/scope-authority'
import type { ScopeClaim } from '../core/scope-authority'
import { useProjectRegisterStore } from '../stores/project-register'
import { usePlanEditorStore } from '../stores/plan-editor'
import { usePlanDiagnosticsStore } from '../stores/plan-diagnostics'
import { useCoverageMatrixStore } from '../stores/coverage-matrix'
import { useGraphProposalsStore } from '../stores/graph-proposals'
import { useProposalCoverageStore } from '../stores/proposal-coverage'
import { useTicketDialogStore } from '../stores/ticket-dialog'
import AppButton from '../components/AppButton.vue'
import EmptyState from '../components/EmptyState.vue'
import ReviewConfigEditor from '../components/ReviewConfigEditor.vue'
import ScheduleEditor from '../components/ScheduleEditor.vue'
import InlineAlert from '../components/InlineAlert.vue'
import SectionHeader from '../components/SectionHeader.vue'
import StatusBadge from '../components/StatusBadge.vue'

const transport = inject(kanbanTransportKey)
const route = useRoute()
const projects = useProjectRegisterStore()
const editor = usePlanEditorStore()
const diagnostics = usePlanDiagnosticsStore()
const matrix = useCoverageMatrixStore()
const graphs = useGraphProposalsStore()
const proposalCoverage = useProposalCoverageStore()
const ticketDialog = useTicketDialogStore()

const pickedProjectId = ref<number | null>(null)
const specDraft = ref('')
const edgeFrom = ref<number | null>(null)
const edgeTo = ref<number | null>(null)
const tickets = ref<TicketRecord[]>([])
const ticketsError = ref<string | null>(null)

function linked(name: string): number | null {
  const raw = route.query[name]
  const value = Array.isArray(raw) ? raw[0] : raw
  const parsed = Number(value)
  return value && Number.isInteger(parsed) && parsed > 0 ? parsed : null
}

const linkedProjectId = computed(() => linked('project'))
const linkedPlanId = computed(() => linked('plan'))
const linkedSpecId = computed(() => linked('spec'))

// Vue Router reuses this component when only the query changes.
const scope = emptyScope()
const specScope = emptyScope()

const loadedProjectId = ref<number | null>(null)

watch(
  () => [linkedProjectId.value, linkedPlanId.value, linkedSpecId.value] as const,
  () => {
    void adoptRoute()
  },
  { immediate: true },
)

async function adoptRoute(): Promise<void> {
  const claim = adoptScope(
    scope,
    `planning:${linkedProjectId.value}:${linkedPlanId.value}:${linkedSpecId.value}`,
  )
  const intended = enforceRouteScope()
  if (!transport) return
  await projects.refresh(transport)
  if (!scopeHolds(scope, claim)) return
  const chosen =
    projects.projects.find((project) => project.id === intended) ??
    projects.projects.find((project) => !project.archived) ??
    projects.projects[0]
  if (!chosen) return
  const linkedObjects = { plan: linkedPlanId.value, spec: linkedSpecId.value }
  if (chosen.id === loadedProjectId.value) {
    pickedProjectId.value = chosen.id
    await openLinked(claim, linkedObjects)
    return
  }
  if (chosen.id !== editor.projectId) {
    forgetProjectScope()
  }
  pickedProjectId.value = chosen.id
  loadedProjectId.value = chosen.id
  await openProject(claim, chosen.id, linkedObjects)
}

// Route authority is applied before any read can yield.
function enforceRouteScope(): number | null {
  const named = linkedProjectId.value
  const held = editor.projectId
  const intended = named ?? held ?? pickedProjectId.value
  if (intended !== held) {
    forgetProjectScope()
    loadedProjectId.value = null
  } else {
    const namedPlan = linkedPlanId.value
    if (namedPlan !== null && editor.selectedPlanId !== namedPlan) {
      editor.forgetSelection()
    }
    const namedSpec = linkedSpecId.value
    if (namedSpec !== null && matrix.pickedSpecId !== namedSpec) {
      releaseScope(specScope)
      matrix.forgetPick()
      graphs.clear()
      proposalCoverage.clear()
    }
  }
  pickedProjectId.value = intended
  return intended
}

function forgetProjectScope(): void {
  specDraft.value = ''
  edgeFrom.value = null
  edgeTo.value = null
  tickets.value = []
  ticketsError.value = null
  editor.clear()
  matrix.clear()
  graphs.clear()
  proposalCoverage.clear()
  diagnostics.clear()
  releaseScope(specScope)
}

async function openProject(
  claim: ScopeClaim,
  projectId: number,
  named: { plan: number | null; spec: number | null } = { plan: null, spec: null },
): Promise<void> {
  if (!transport) return
  await Promise.all([
    editor.refresh(transport, projectId),
    matrix.loadSpecs(transport, projectId),
    loadTickets(claim, projectId),
  ])
  if (!scopeHolds(scope, claim)) return
  await openLinked(claim, named)
}

async function openLinked(
  claim: ScopeClaim,
  named: { plan: number | null; spec: number | null },
): Promise<void> {
  if (!transport) return
  if (named.plan !== null && editor.plans.some((plan) => plan.id === named.plan)) {
    await editor.open(transport, named.plan)
    if (!scopeHolds(scope, claim)) return
  }
  if (named.spec !== null && matrix.specs.some((spec) => spec.id === named.spec)) {
    await matrix.pick(transport, named.spec)
    if (!scopeHolds(scope, claim)) return
  }
  const specId = matrix.pickedSpecId
  if (specId === null) {
    releaseScope(specScope)
    graphs.clear()
    proposalCoverage.clear()
    return
  }
  const specClaim = takeSpecAuthority(specId)
  await loadGraphs(specClaim, specId)
}

async function loadTickets(
  claim: ScopeClaim,
  projectId: number,
  specClaim: ScopeClaim | null = null,
): Promise<void> {
  if (!transport) return
  try {
    const response = await new KanbanClient(transport).queryTicketList({ project_id: projectId })
    if (!scopeHolds(scope, claim) || (specClaim && !scopeHolds(specScope, specClaim))) return
    tickets.value = response.tickets ?? []
    ticketsError.value = null
  } catch (failure) {
    if (!scopeHolds(scope, claim) || (specClaim && !scopeHolds(specScope, specClaim))) return
    tickets.value = []
    ticketsError.value = asApiError(failure).message
  }
}

function takeSpecAuthority(specId: number): ScopeClaim {
  const claim = adoptScope(specScope, `planning-spec:${pickedProjectId.value}:${specId}`)
  graphs.clear()
  proposalCoverage.clear()
  return claim
}

async function loadGraphs(claim: ScopeClaim, specId: number): Promise<void> {
  if (!transport) return
  await graphs.load(transport, specId)
  if (!scopeHolds(specScope, claim) || graphs.specId !== specId) return
  await proposalCoverage.load(
    transport,
    specId,
    graphs.proposals.map((entry) => entry.spec_version),
  )
}

async function loadPlans(): Promise<void> {
  if (!transport || pickedProjectId.value === null) return
  const chosen = pickedProjectId.value
  const claim = adoptScope(scope, `planning:picked:${chosen}`)
  forgetProjectScope()
  pickedProjectId.value = chosen
  loadedProjectId.value = chosen
  await openProject(claim, chosen)
}

const projectCode = computed(
  () => projects.projects.find((project) => project.id === pickedProjectId.value)?.code ?? '',
)

function planId(plan: { number: number }): string {
  return `${projectCode.value}-P${plan.number}`
}

function specId(spec: number): string {
  return `${projectCode.value}-S${spec}`
}

function ticketId(ticket: number): string {
  return `${projectCode.value}-T${ticket}`
}

// A Story no Ticket claims is covered by an Implementation on this
// Spec: the editor opens as a dialog, preset to that kind and that
// Story, and nothing here mutates until the dialog's own command
// lands (KAN-T139-AC1).
function coverStory(story: string): void {
  if (pickedProjectId.value === null || matrix.pickedSpecId === null) return
  ticketDialog.openForStory({
    projectId: pickedProjectId.value,
    specId: matrix.pickedSpecId,
    story,
  })
}

// A saved schedule changes the Ticket the editors read, so the
// Project's Tickets are read again.
async function scheduleSaved(): Promise<void> {
  if (pickedProjectId.value === null) return
  const claim = adoptScope(scope, `planning:picked:${pickedProjectId.value}`)
  await loadTickets(claim, pickedProjectId.value)
}

function memberLabel(id: number): string {
  const held = tickets.value.find((entry) => entry.id === id)
  if (!held) return `Ticket ${id}`
  const identity = ticketId(held.number)
  return held.pinned_spec_version == null
    ? identity
    : `${identity} · pinned v${held.pinned_spec_version}`
}

async function pickSpec(): Promise<void> {
  if (transport && matrix.pickedSpecId !== null) {
    const specId = matrix.pickedSpecId
    const claim = takeSpecAuthority(specId)
    await matrix.pick(transport, specId)
    if (!scopeHolds(specScope, claim)) return
    await loadGraphs(claim, specId)
  }
}

// Approval coverage is pinned to the proposal's version, not the operative one.
function proposalBasis(version: number) {
  return proposalCoverage.coverageOf(version)
}

async function approveGraph(proposalId: number): Promise<void> {
  const proposal = graphs.proposals.find((entry) => entry.id === proposalId)
  const projectId = pickedProjectId.value
  const specId = matrix.pickedSpecId
  if (!transport || !proposal || projectId === null || specId !== proposal.spec_id) return
  const claim = issueCommand(scope)
  const specClaim = issueCommand(specScope)
  const landed = await graphs.approve(transport, proposal)
  if (!scopeHolds(scope, claim) || !scopeHolds(specScope, specClaim) || !landed) return
  await loadTickets(claim, projectId, specClaim)
  if (!scopeHolds(scope, claim) || !scopeHolds(specScope, specClaim)) return
  await proposalCoverage.load(
    transport,
    specId,
    graphs.proposals.map((entry) => entry.spec_version),
  )
  if (!scopeHolds(scope, claim) || !scopeHolds(specScope, specClaim)) return
  await matrix.read(transport, specId)
}

const selected = computed(() => editor.selectedPlan)

const displayed = computed(() => editor.displayed)

const editable = computed(
  () => selected.value?.state === 'draft' && editor.selectedVersion === null,
)

const switcher = computed(() => [
  { key: 'draft' as const, label: 'Draft' },
  ...[...editor.versions].reverse().map((version) => ({
    key: version.number as number | 'draft',
    label: `v${version.number}`,
  })),
])

async function submitCreate(): Promise<void> {
  if (transport && pickedProjectId.value !== null) {
    await editor.create(transport, pickedProjectId.value)
  }
}

async function submitAddSpec(): Promise<void> {
  if (!transport || !editable.value) {
    return
  }
  const number = Number.parseInt(specDraft.value, 10)
  if (Number.isInteger(number) && number > 0) {
    await editor.addSpec(transport, number)
    specDraft.value = ''
  }
}

async function removeSpec(spec: number): Promise<void> {
  if (transport && editable.value) {
    await editor.removeSpec(transport, spec)
  }
}

async function moveSpec(spec: number, position: number): Promise<void> {
  if (transport && editable.value && position >= 0) {
    await editor.moveSpec(transport, spec, position)
  }
}

async function submitAddEdge(): Promise<void> {
  if (!transport || !editable.value || edgeFrom.value === null || edgeTo.value === null) {
    return
  }
  await editor.addEdge(transport, edgeFrom.value, edgeTo.value)
  edgeFrom.value = null
  edgeTo.value = null
}

async function removeEdge(from: number, to: number): Promise<void> {
  if (transport && editable.value) {
    await editor.removeEdge(transport, from, to)
  }
}

async function lifecycle(action: 'activate' | 'replan' | 'complete' | 'cancel' | 'archive') {
  if (!transport) {
    return
  }
  await editor[action](transport)
}

// One stable key for the graph on display: the open Plan, the
// displayed version, and the working shape's stored version, which
// every applied edit bumps. A string keeps the watcher quiet when the
// Plan list re-renders without the displayed graph changing.
const displayedGraphKey = computed(
  () =>
    `${editor.selectedPlan?.id ?? 'none'}-${editor.selectedVersion ?? 'draft'}-${editor.selectedPlan?.version ?? 0}`,
)

// The diagnostics follow the graph on display: re-read them whenever
// that key changes and at mount — a Spec content edit or a return
// from another view leaves the retained selection's diagnostics
// stale even though the displayed graph key never changed — and
// forget them when no Plan is open (KAN-S3-US7).
watch(
  displayedGraphKey,
  () => {
    const planId = editor.selectedPlan?.id ?? null
    if (transport && planId !== null) {
      void diagnostics.refresh(transport, planId, editor.selectedVersion)
    } else {
      diagnostics.clear()
    }
  },
  { immediate: true },
)

const blocking = computed(() => diagnostics.report?.blocking ?? false)

const stateLabels: Record<string, string> = {
  draft: 'draft',
  active: 'active',
  complete: 'complete',
  cancelled: 'cancelled',
  archived: 'archived',
}
</script>

<template>
  <main class="animate-rise flex flex-col gap-6 px-6 py-8 lg:px-8">
    <SectionHeader
      eyebrow="Authoring"
      title="Plan the work"
      summary="Plans are ordered dependency graphs of Specs. The diagnostics beside a graph say whether it can become executable, and the Ticket graphs proposed against a Spec meet their approval gate here."
    >
      <template #actions>
        <form @submit.prevent="submitCreate">
          <AppButton
            type="submit"
            data-testid="plan-create"
            size="sm"
            variant="primary"
          >
            New Plan
          </AppButton>
        </form>
      </template>
    </SectionHeader>

    <label class="flex w-fit max-w-full flex-col gap-1 text-sm text-ink-muted">
      Project
      <select
        v-model="pickedProjectId"
        data-testid="planning-project"
        aria-label="Project"
        class="min-w-0 max-w-full rounded-control border border-line bg-surface px-3 py-2 text-sm text-ink"
        @change="loadPlans"
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

    <InlineAlert
      v-if="editor.error"
      data-testid="plan-error"
    >
      {{ editor.error }}
    </InlineAlert>

    <section
      v-if="editor.loaded"
      data-testid="plan-list"
      class="flex flex-col gap-4"
    >
      <div>
        <h2 class="mb-2 text-xs font-semibold tracking-[0.12em] text-ink-subtle uppercase">
          Active surface
        </h2>
        <ul
          data-testid="plan-active"
          class="flex flex-col divide-y divide-line overflow-hidden rounded-panel border border-line bg-surface"
        >
          <li
            v-for="plan in editor.activeSurface"
            :key="plan.id"
            :data-testid="`plan-row-${plan.id}`"
            class="flex cursor-pointer items-center gap-3 px-4 py-3 transition-colors hover:bg-accent/6"
            @click="editor.open(transport!, plan.id)"
          >
            <span class="rounded-control bg-rail px-2 py-0.5 font-mono text-sm font-medium text-ink">
              {{ planId(plan) }}
            </span>
            <span
              :data-testid="`plan-state-${plan.id}`"
              class="rounded-control bg-rail px-2 py-0.5 text-xs text-ink-muted"
            >{{ stateLabels[plan.state] }}</span>
            <span class="font-mono text-xs text-ink-subtle">
              {{ plan.spec_numbers.length }} Specs · {{ plan.edges.length }} edges
            </span>
          </li>
        </ul>
      </div>

      <div v-if="editor.finished.length">
        <h2 class="mb-2 text-xs font-semibold tracking-[0.12em] text-ink-subtle uppercase">
          Finished
        </h2>
        <ul
          data-testid="plan-finished"
          class="flex flex-col divide-y divide-line overflow-hidden rounded-panel border border-line bg-surface"
        >
          <li
            v-for="plan in editor.finished"
            :key="plan.id"
            :data-testid="`plan-row-${plan.id}`"
            class="flex cursor-pointer items-center gap-3 px-4 py-3 text-ink-muted transition-colors hover:bg-accent/6"
            @click="editor.open(transport!, plan.id)"
          >
            <span class="rounded-control bg-rail px-2 py-0.5 font-mono text-sm font-medium">
              {{ planId(plan) }}
            </span>
            <span class="rounded-control bg-rail px-2 py-0.5 text-xs">
              {{ stateLabels[plan.state] }}
            </span>
            <span class="font-mono text-xs">
              {{ plan.spec_numbers.length }} Specs · {{ plan.edges.length }} edges
            </span>
          </li>
        </ul>
      </div>
    </section>
    <p
      v-else-if="!editor.error"
      data-testid="plan-loading"
      class="text-sm text-ink-subtle"
    >
      Loading Plans…
    </p>

    <section
      v-if="matrix.specs.length"
      data-testid="planning-specs"
      class="flex flex-col gap-4 rounded-panel border border-line bg-surface p-4"
    >
      <header class="flex flex-wrap items-center gap-3">
        <h3 class="text-sm font-semibold text-ink">
          Specs
        </h3>
        <select
          v-model="matrix.pickedSpecId"
          data-testid="coverage-spec"
          aria-label="Spec"
          class="min-w-0 max-w-full rounded-control border border-line bg-surface px-3 py-1.5 text-sm text-ink"
          @change="pickSpec"
        >
          <option
            v-for="spec in matrix.specs"
            :key="spec.id"
            :value="spec.id"
          >
            {{ specId(spec.number) }} — {{ spec.name }}
          </option>
        </select>
        <span
          v-if="matrix.report"
          data-testid="coverage-version"
          class="rounded-control bg-rail px-2 py-0.5 font-mono text-xs text-ink-muted"
        >v{{ matrix.report.version }}</span>
      </header>

      <ul
        data-testid="planning-spec-list"
        class="flex flex-wrap gap-2"
      >
        <li
          v-for="spec in matrix.specs"
          :key="spec.id"
          :data-testid="`planning-spec-${spec.id}`"
          class="rounded-control border px-2.5 py-1 font-mono text-xs"
          :class="spec.id === matrix.pickedSpecId
            ? 'border-accent/40 bg-accent/9 text-accent'
            : 'border-line text-ink-muted'"
        >
          {{ specId(spec.number) }} · {{ spec.execution }}
        </li>
      </ul>

      <div
        data-testid="coverage-matrix"
        class="flex flex-col gap-3"
      >
        <h4 class="text-xs font-semibold tracking-[0.12em] text-ink-subtle uppercase">
          Coverage matrix
        </h4>
        <InlineAlert
          v-if="matrix.error"
          data-testid="coverage-error"
        >
          {{ matrix.error }}
        </InlineAlert>

        <ul
          v-if="matrix.report"
          data-testid="coverage-rows"
          class="flex flex-col divide-y divide-line overflow-hidden rounded-control border border-line"
        >
          <li
            v-for="row in matrix.report.stories"
            :key="row.story"
            :data-testid="`coverage-row-${row.story}`"
            class="flex flex-col gap-1 px-3 py-2"
          >
            <div class="flex items-center gap-2">
              <span class="font-mono text-sm text-ink">{{ row.story }}</span>
              <StatusBadge
                v-if="row.claims.length === 0"
                :data-testid="`coverage-gap-${row.story}`"
                tone="critical"
                density="compact"
              >
                uncovered
              </StatusBadge>
              <AppButton
                v-if="row.claims.length === 0"
                size="sm"
                :data-testid="`coverage-cover-${row.story}`"
                :aria-label="`Cover ${row.story} with an Implementation Ticket`"
                @click="coverStory(row.story)"
              >
                Cover with a Ticket
              </AppButton>
            </div>
            <ul
              v-if="row.claims.length"
              class="flex flex-col gap-0.5"
            >
              <li
                v-for="claim in row.claims"
                :key="`${claim.ticket_id}-${claim.outcome}`"
                :data-testid="`coverage-claim-${row.story}-${claim.ticket_number}`"
                class="text-sm text-ink-muted"
              >
                <span class="font-mono">{{ ticketId(claim.ticket_number) }}</span>
                — {{ claim.outcome }}
              </li>
            </ul>
          </li>
        </ul>
        <p
          v-else-if="!matrix.error"
          data-testid="coverage-loading"
          class="text-sm text-ink-subtle"
        >
          Loading the coverage matrix…
        </p>
      </div>

      <div
        data-testid="graph-proposals"
        class="flex flex-col gap-3"
      >
        <h4 class="text-xs font-semibold tracking-[0.12em] text-ink-subtle uppercase">
          Ticket graph approval
        </h4>
        <InlineAlert
          v-if="graphs.error"
          data-testid="graph-error"
        >
          {{ graphs.error }}
        </InlineAlert>
        <InlineAlert
          v-if="ticketsError"
          data-testid="graph-tickets-error"
        >
          {{ ticketsError }}
        </InlineAlert>
        <ul
          v-if="graphs.proposals.length"
          class="flex flex-col divide-y divide-line overflow-hidden rounded-control border border-line"
        >
          <li
            v-for="entry in graphs.proposals"
            :key="entry.id"
            :data-testid="`graph-proposal-${entry.id}`"
            class="flex flex-col gap-2 px-3 py-3"
          >
            <div class="flex flex-wrap items-center gap-2">
              <span class="font-mono text-sm text-ink">Graph {{ entry.id }}</span>
              <span class="rounded-control bg-rail px-2 py-0.5 font-mono text-xs text-ink-muted">
                v{{ entry.spec_version }}
              </span>
              <StatusBadge
                :data-testid="`graph-state-${entry.id}`"
                :tone="entry.state === 'approved' ? 'positive' : 'caution'"
                density="compact"
              >
                {{ entry.state }}
              </StatusBadge>
              <AppButton
                v-if="entry.state === 'proposed'"
                :data-testid="`graph-approve-${entry.id}`"
                size="sm"
                variant="primary"
                class="ml-auto"
                @click="approveGraph(entry.id)"
              >
                Approve graph
              </AppButton>
            </div>
            <p
              :data-testid="`graph-members-${entry.id}`"
              class="text-sm text-ink-muted"
            >
              {{ entry.tickets.map((member) => memberLabel(member)).join(', ') }}
            </p>
            <p
              :data-testid="`graph-edges-${entry.id}`"
              class="font-mono text-xs text-ink-subtle"
            >
              {{ entry.edges
                .map((edge) => `${memberLabel(edge.from_ticket).split(' · ')[0]} → ${memberLabel(edge.to_ticket).split(' · ')[0]}`)
                .join(', ') }}
            </p>
            <InlineAlert
              v-if="proposalBasis(entry.spec_version)?.error"
              :data-testid="`graph-coverage-error-${entry.id}`"
            >
              The coverage of v{{ entry.spec_version }}, which this graph is judged on, could not
              be read: {{ proposalBasis(entry.spec_version)!.error }}
            </InlineAlert>
            <InlineAlert
              v-else-if="proposalBasis(entry.spec_version)?.uncovered.length"
              :data-testid="`graph-blocking-${entry.id}`"
              tone="caution"
            >
              The gate refuses a graph that leaves a story unclaimed.
              {{ proposalBasis(entry.spec_version)!.uncovered.join(', ') }}
              {{ proposalBasis(entry.spec_version)!.uncovered.length === 1 ? 'is' : 'are' }}
              claimed by no criterion of v{{ entry.spec_version }}, the version this graph names.
            </InlineAlert>
            <p
              v-else-if="proposalBasis(entry.spec_version)"
              :data-testid="`graph-covered-${entry.id}`"
              class="text-xs text-ink-subtle"
            >
              Every story of v{{ entry.spec_version }} is claimed by a criterion.
            </p>
            <InlineAlert
              v-if="graphs.refusal?.proposalId === entry.id"
              :data-testid="`graph-refusal-${entry.id}`"
            >
              {{ graphs.refusal.message }}
            </InlineAlert>
          </li>
        </ul>
        <EmptyState
          v-else-if="graphs.loaded"
          compact
          data-testid="graph-empty"
          message="No Ticket graph has been proposed against this Spec."
          hint="An agent records a complete graph against an approved Spec version; the gate here is the human decision on it."
        />
      </div>
    </section>

    <section
      v-if="selected && displayed"
      data-testid="plan-editor"
      class="flex flex-col gap-4 rounded-panel border border-line bg-surface p-4"
    >
      <header class="flex flex-wrap items-center gap-3">
        <h3
          data-testid="plan-title"
          class="font-mono text-lg font-semibold text-ink"
        >
          {{ planId(selected) }}
        </h3>
        <span
          data-testid="plan-state"
          class="rounded-control bg-rail px-2 py-0.5 text-xs text-ink-muted"
        >{{ stateLabels[selected.state] }}</span>
        <div class="ml-auto flex flex-wrap items-center gap-2">
          <div
            data-testid="plan-versions"
            class="flex items-center gap-1"
          >
            <button
              v-for="entry in switcher"
              :key="entry.key"
              :data-testid="`plan-version-${entry.key}`"
              type="button"
              class="rounded-control border border-line px-2 py-1 font-mono text-xs transition-colors hover:bg-accent/8"
              :class="entry.key === 'draft'
                ? (editor.selectedVersion === null ? 'bg-accent/12 text-accent' : 'text-ink-muted')
                : (editor.selectedVersion === entry.key ? 'bg-accent/12 text-accent' : 'text-ink-muted')"
              @click="entry.key === 'draft' ? editor.showDraft() : editor.showVersion(entry.key as number)"
            >
              {{ entry.label }}
            </button>
          </div>
        </div>
      </header>

      <div class="flex flex-wrap gap-2">
        <form
          v-if="selected.state === 'draft'"
          @submit.prevent="lifecycle('activate')"
        >
          <AppButton
            type="submit"
            data-testid="plan-activate"
            size="sm"
            variant="primary"
          >
            Activate
          </AppButton>
        </form>
        <form
          v-if="selected.state === 'active'"
          @submit.prevent="lifecycle('replan')"
        >
          <AppButton
            type="submit"
            data-testid="plan-replan"
            size="sm"
            variant="primary"
          >
            Replan
          </AppButton>
        </form>
        <form
          v-if="selected.state === 'active'"
          @submit.prevent="lifecycle('complete')"
        >
          <AppButton
            type="submit"
            data-testid="plan-complete"
            size="sm"
          >
            Complete
          </AppButton>
        </form>
        <form
          v-if="selected.state === 'draft' || selected.state === 'active'"
          @submit.prevent="lifecycle('cancel')"
        >
          <AppButton
            type="submit"
            data-testid="plan-cancel"
            size="sm"
          >
            Cancel
          </AppButton>
        </form>
        <form
          v-if="selected.state !== 'archived'"
          @submit.prevent="lifecycle('archive')"
        >
          <AppButton
            type="submit"
            data-testid="plan-archive"
            size="sm"
          >
            Archive
          </AppButton>
        </form>
      </div>

      <p
        v-if="!editable"
        data-testid="plan-readonly"
        class="text-xs text-ink-subtle"
      >
        {{ editor.selectedVersion === null
          ? 'Only a draft Plan accepts shape edits.'
          : `Viewing frozen version v${editor.selectedVersion}; switch to Draft to edit.` }}
      </p>

      <section
        v-if="diagnostics.loaded || diagnostics.error"
        data-testid="plan-diagnostics"
        class="flex flex-col gap-2 rounded-control border p-4"
        :class="blocking ? 'border-critical/30 bg-critical/8' : 'border-line'"
        :aria-label="editor.selectedVersion === null
          ? `Diagnostics of the working shape`
          : `Diagnostics of frozen version v${editor.selectedVersion}`"
      >
        <h4
          class="text-sm font-semibold"
          :class="blocking ? 'text-critical' : 'text-ink'"
        >
          Diagnostics
        </h4>
        <p
          v-if="diagnostics.error"
          data-testid="plan-diagnostics-error"
          role="alert"
          class="text-sm text-critical"
        >
          {{ diagnostics.error }}
        </p>
        <template v-else-if="diagnostics.report">
          <p
            v-if="blocking"
            data-testid="plan-diagnostics-blocking"
            class="text-sm font-medium text-critical"
          >
            This graph is blocked: it cannot become executable yet.
          </p>
          <p
            v-else
            data-testid="plan-diagnostics-clear"
            class="text-sm text-ink-muted"
          >
            No blocking diagnostics.
          </p>
          <ul
            v-if="diagnostics.report.cycles.length"
            data-testid="plan-diagnostics-cycles"
            class="flex flex-col gap-1"
          >
            <li
              v-for="(cycle, index) in diagnostics.report.cycles"
              :key="`cycle-${index}`"
              :data-testid="`plan-diagnostics-cycle-${index}`"
              class="text-sm text-critical"
            >
              {{ cycle.spec_numbers.map((spec) => specId(spec)).join(' → ') }}
              form a dependency cycle.
            </li>
          </ul>
          <ul
            v-if="diagnostics.report.coverage_gaps.length"
            data-testid="plan-diagnostics-gaps"
            class="flex flex-col gap-1"
          >
            <li
              v-for="gap in diagnostics.report.coverage_gaps"
              :key="`gap-${gap.spec_number}`"
              :data-testid="`plan-diagnostics-gap-${gap.spec_number}`"
              class="text-sm text-critical"
            >
              {{ gap.claims_no_stories
                ? `${specId(gap.spec_number)} claims no User Stories to cover.`
                : `${specId(gap.spec_number)}: ${gap.uncovered.join(', ')} uncovered.` }}
            </li>
          </ul>
          <ul
            v-if="diagnostics.report.invalid_profiles.length"
            data-testid="plan-diagnostics-profiles"
            class="flex flex-col gap-1"
          >
            <li
              v-for="(profile, index) in diagnostics.report.invalid_profiles"
              :key="`profile-${index}`"
              :data-testid="`plan-diagnostics-profile-${index}`"
              class="text-sm text-critical"
            >
              Profile reference {{ profile.reference }} resolves to no catalogue entry.
            </li>
          </ul>
        </template>
      </section>

      <div class="grid gap-4 md:grid-cols-2">
        <section class="flex flex-col gap-2">
          <h4 class="text-xs font-semibold tracking-[0.12em] text-ink-subtle uppercase">
            Display order
          </h4>
          <ol
            data-testid="plan-specs"
            class="flex flex-col divide-y divide-line overflow-hidden rounded-control border border-line"
          >
            <li
              v-for="(spec, position) in displayed.spec_numbers"
              :key="spec"
              :data-testid="`plan-spec-row-${spec}`"
              class="flex items-center gap-2 px-3 py-2 text-sm text-ink"
            >
              <span class="font-mono">{{ specId(spec) }}</span>
              <span class="ml-auto flex items-center gap-1">
                <button
                  :data-testid="`plan-spec-up-${spec}`"
                  type="button"
                  :disabled="!editable || position === 0"
                  class="rounded-control border border-line px-2 py-0.5 text-xs text-ink-muted disabled:opacity-30"
                  @click="moveSpec(spec, position - 1)"
                >
                  ↑
                </button>
                <button
                  :data-testid="`plan-spec-down-${spec}`"
                  type="button"
                  :disabled="!editable || position === displayed.spec_numbers.length - 1"
                  class="rounded-control border border-line px-2 py-0.5 text-xs text-ink-muted disabled:opacity-30"
                  @click="moveSpec(spec, position + 1)"
                >
                  ↓
                </button>
                <button
                  :data-testid="`plan-spec-remove-${spec}`"
                  type="button"
                  :disabled="!editable"
                  class="rounded-control border border-line px-2 py-0.5 text-xs text-ink-muted disabled:opacity-30"
                  @click="removeSpec(spec)"
                >
                  Remove
                </button>
              </span>
            </li>
          </ol>
          <form
            class="flex items-center gap-2"
            @submit.prevent="submitAddSpec"
          >
            <input
              v-model="specDraft"
              data-testid="plan-spec-number"
              aria-label="Spec number"
              placeholder="Spec number, for example 4"
              class="w-52 rounded-control border border-line bg-surface px-3 py-2 text-sm text-ink"
            >
            <AppButton
              type="submit"
              data-testid="plan-spec-add"
              size="sm"
              :disabled="!editable"
            >
              Add Spec
            </AppButton>
          </form>
        </section>

        <section class="flex flex-col gap-2">
          <h4 class="text-xs font-semibold tracking-[0.12em] text-ink-subtle uppercase">
            Dependency edges
          </h4>
          <ul
            data-testid="plan-edges"
            class="flex flex-col divide-y divide-line overflow-hidden rounded-control border border-line"
          >
            <li
              v-for="edge in displayed.edges"
              :key="`${edge.from_spec}-${edge.to_spec}`"
              :data-testid="`plan-edge-row-${edge.from_spec}-${edge.to_spec}`"
              class="flex items-center gap-2 px-3 py-2 text-sm text-ink"
            >
              <span class="font-mono">
                {{ specId(edge.from_spec) }} → {{ specId(edge.to_spec) }}
              </span>
              <button
                :data-testid="`plan-edge-remove-${edge.from_spec}-${edge.to_spec}`"
                type="button"
                :disabled="!editable"
                class="ml-auto rounded-control border border-line px-2 py-0.5 text-xs text-ink-muted disabled:opacity-30"
                @click="removeEdge(edge.from_spec, edge.to_spec)"
              >
                Remove
              </button>
            </li>
          </ul>
          <form
            class="flex items-center gap-2"
            @submit.prevent="submitAddEdge"
          >
            <select
              v-model="edgeFrom"
              data-testid="plan-edge-from"
              aria-label="Depends on"
              class="rounded-control border border-line bg-surface px-2 py-2 text-sm text-ink"
            >
              <option :value="null">
                from
              </option>
              <option
                v-for="spec in displayed.spec_numbers"
                :key="`from-${spec}`"
                :value="spec"
              >
                {{ specId(spec) }}
              </option>
            </select>
            <select
              v-model="edgeTo"
              data-testid="plan-edge-to"
              aria-label="Waits on"
              class="rounded-control border border-line bg-surface px-2 py-2 text-sm text-ink"
            >
              <option :value="null">
                to
              </option>
              <option
                v-for="spec in displayed.spec_numbers"
                :key="`to-${spec}`"
                :value="spec"
              >
                {{ specId(spec) }}
              </option>
            </select>
            <AppButton
              type="submit"
              data-testid="plan-edge-add"
              size="sm"
              :disabled="!editable"
            >
              Add edge
            </AppButton>
          </form>
        </section>
      </div>
    </section>

    <!-- Per-Ticket execution configuration: a Task's activation
         schedule and an assignment's review stages. Neither edits a
         Ticket's own content, so neither belongs in the Ticket editor
         dialog; both belong beside the Project's Tickets. -->
    <ScheduleEditor
      v-if="tickets.length > 0"
      :key="`schedule-${pickedProjectId ?? 'none'}`"
      :tickets="tickets"
      :project-code="projectCode"
      @saved="scheduleSaved"
    />

    <ReviewConfigEditor
      v-if="tickets.length > 0"
      :key="`reviews-${pickedProjectId ?? 'none'}`"
      :tickets="tickets"
      :project-code="projectCode"
    />
  </main>
</template>
