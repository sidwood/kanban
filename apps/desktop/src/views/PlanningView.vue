<script setup lang="ts">
// The planning editor surface: pick a Project, compose its Plans as
// ordered dependency graphs of Specs, drive the lifecycle, and switch
// between the working shape and every frozen version. Presentation
// only; every domain call goes through the generated client in the
// plan-editor and plan-diagnostics stores, and the terminal states
// stay listed but sit off the active surface (KAN-S3-US1, KAN-S3-US2,
// KAN-S3-US3). The blocking diagnostics of the graph on display ride
// beside it (KAN-S3-US7).
import { computed, inject, onMounted, ref, watch } from 'vue'
import { kanbanTransportKey } from '../core/transport'
import { useProjectRegisterStore } from '../stores/project-register'
import { usePlanEditorStore } from '../stores/plan-editor'
import { usePlanDiagnosticsStore } from '../stores/plan-diagnostics'

const transport = inject(kanbanTransportKey)
const projects = useProjectRegisterStore()
const editor = usePlanEditorStore()
const diagnostics = usePlanDiagnosticsStore()

const pickedProjectId = ref<number | null>(null)
const specDraft = ref('')
const edgeFrom = ref<number | null>(null)
const edgeTo = ref<number | null>(null)

onMounted(() => {
  if (transport) {
    void projects.refresh(transport).then(() => {
      const first = projects.projects.find((project) => !project.archived) ?? projects.projects[0]
      if (first) {
        pickedProjectId.value = first.id
        void editor.refresh(transport, first.id)
      }
    })
  }
})

// Loading the picked Project's Plans.
async function loadPlans(): Promise<void> {
  if (transport && pickedProjectId.value !== null) {
    editor.select(0)
    await editor.refresh(transport, pickedProjectId.value)
  }
}

// The code of the picked Project, for rendering Plan identities.
const projectCode = computed(
  () => projects.projects.find((project) => project.id === pickedProjectId.value)?.code ?? '',
)

function planId(plan: { number: number }): string {
  return `${projectCode.value}-P${plan.number}`
}

function specId(spec: number): string {
  return `${projectCode.value}-S${spec}`
}

// The Plan the editor has open.
const selected = computed(() => editor.selectedPlan)

// The graph on display: a frozen version's or the working shape's.
const displayed = computed(() => editor.displayed)

// Editing is legal only while a draft is on display.
const editable = computed(
  () => selected.value?.state === 'draft' && editor.selectedVersion === null,
)

// The version switcher's entries: the working shape plus every frozen
// version, newest first.
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

// Whether the graph on display carries a blocking diagnostic.
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
  <main class="mx-auto flex min-h-screen max-w-4xl flex-col gap-6 p-8">
    <nav class="text-sm text-slate-500">
      <RouterLink
        to="/"
        class="hover:text-slate-900"
      >
        Kanban
      </RouterLink>
      <span aria-hidden="true"> / </span>
      <span class="text-slate-900">Planning</span>
    </nav>

    <h1 class="text-3xl font-semibold tracking-tight">
      Plan the work
    </h1>

    <div class="flex items-end gap-3">
      <label class="flex flex-col gap-1 text-sm text-slate-600">
        Project
        <select
          v-model="pickedProjectId"
          data-testid="planning-project"
          aria-label="Project"
          class="rounded border border-slate-300 px-3 py-2 text-sm"
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
      <form @submit.prevent="submitCreate">
        <button
          type="submit"
          data-testid="plan-create"
          class="rounded bg-slate-900 px-3 py-2 text-sm font-medium text-white hover:bg-slate-700"
        >
          New Plan
        </button>
      </form>
    </div>

    <p
      v-if="editor.error"
      data-testid="plan-error"
      role="alert"
      class="rounded border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-700"
    >
      {{ editor.error }}
    </p>

    <section
      v-if="editor.loaded"
      data-testid="plan-list"
      class="flex flex-col gap-4"
    >
      <div>
        <h2 class="mb-2 text-sm font-semibold text-slate-700">
          Active surface
        </h2>
        <ul
          data-testid="plan-active"
          class="flex flex-col divide-y divide-slate-200 rounded border border-slate-200"
        >
          <li
            v-for="plan in editor.activeSurface"
            :key="plan.id"
            :data-testid="`plan-row-${plan.id}`"
            class="flex cursor-pointer items-center gap-3 px-4 py-3 hover:bg-slate-50"
            @click="editor.open(transport!, plan.id)"
          >
            <span class="rounded bg-slate-100 px-2 py-0.5 font-mono text-sm font-medium">
              {{ planId(plan) }}
            </span>
            <span
              :data-testid="`plan-state-${plan.id}`"
              class="rounded bg-slate-100 px-2 py-0.5 text-xs text-slate-600"
            >{{ stateLabels[plan.state] }}</span>
            <span class="font-mono text-xs text-slate-500">
              {{ plan.spec_numbers.length }} Specs · {{ plan.edges.length }} edges
            </span>
          </li>
        </ul>
      </div>

      <div v-if="editor.finished.length">
        <h2 class="mb-2 text-sm font-semibold text-slate-500">
          Finished
        </h2>
        <ul
          data-testid="plan-finished"
          class="flex flex-col divide-y divide-slate-200 rounded border border-slate-100"
        >
          <li
            v-for="plan in editor.finished"
            :key="plan.id"
            :data-testid="`plan-row-${plan.id}`"
            class="flex cursor-pointer items-center gap-3 px-4 py-3 text-slate-500 hover:bg-slate-50"
            @click="editor.open(transport!, plan.id)"
          >
            <span class="rounded bg-slate-50 px-2 py-0.5 font-mono text-sm font-medium">
              {{ planId(plan) }}
            </span>
            <span class="rounded bg-slate-50 px-2 py-0.5 text-xs">
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
    >
      Loading Plans…
    </p>

    <section
      v-if="selected && displayed"
      data-testid="plan-editor"
      class="flex flex-col gap-4 rounded-lg border border-slate-200 p-4"
    >
      <header class="flex flex-wrap items-center gap-3">
        <h3
          data-testid="plan-title"
          class="font-mono text-lg font-semibold"
        >
          {{ planId(selected) }}
        </h3>
        <span
          data-testid="plan-state"
          class="rounded bg-slate-100 px-2 py-0.5 text-xs text-slate-600"
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
              class="rounded border border-slate-300 px-2 py-1 font-mono text-xs hover:bg-slate-50"
              :class="entry.key === 'draft'
                ? (editor.selectedVersion === null ? 'bg-slate-900 text-white' : '')
                : (editor.selectedVersion === entry.key ? 'bg-slate-900 text-white' : '')"
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
          <button
            type="submit"
            data-testid="plan-activate"
            class="rounded bg-slate-900 px-3 py-1.5 text-sm font-medium text-white hover:bg-slate-700"
          >
            Activate
          </button>
        </form>
        <form
          v-if="selected.state === 'active'"
          @submit.prevent="lifecycle('replan')"
        >
          <button
            type="submit"
            data-testid="plan-replan"
            class="rounded bg-slate-900 px-3 py-1.5 text-sm font-medium text-white hover:bg-slate-700"
          >
            Replan
          </button>
        </form>
        <form
          v-if="selected.state === 'active'"
          @submit.prevent="lifecycle('complete')"
        >
          <button
            type="submit"
            data-testid="plan-complete"
            class="rounded border border-slate-300 px-3 py-1.5 text-sm hover:bg-slate-50"
          >
            Complete
          </button>
        </form>
        <form
          v-if="selected.state === 'draft' || selected.state === 'active'"
          @submit.prevent="lifecycle('cancel')"
        >
          <button
            type="submit"
            data-testid="plan-cancel"
            class="rounded border border-slate-300 px-3 py-1.5 text-sm hover:bg-slate-50"
          >
            Cancel
          </button>
        </form>
        <form
          v-if="selected.state !== 'archived'"
          @submit.prevent="lifecycle('archive')"
        >
          <button
            type="submit"
            data-testid="plan-archive"
            class="rounded border border-slate-300 px-3 py-1.5 text-sm hover:bg-slate-50"
          >
            Archive
          </button>
        </form>
      </div>

      <p
        v-if="!editable"
        data-testid="plan-readonly"
        class="text-xs text-slate-500"
      >
        {{ editor.selectedVersion === null
          ? 'Only a draft Plan accepts shape edits.'
          : `Viewing frozen version v${editor.selectedVersion}; switch to Draft to edit.` }}
      </p>

      <section
        v-if="diagnostics.loaded || diagnostics.error"
        data-testid="plan-diagnostics"
        class="flex flex-col gap-2 rounded-lg border p-4"
        :class="blocking ? 'border-red-200 bg-red-50' : 'border-slate-200'"
        :aria-label="editor.selectedVersion === null
          ? `Diagnostics of the working shape`
          : `Diagnostics of frozen version v${editor.selectedVersion}`"
      >
        <h4
          class="text-sm font-semibold"
          :class="blocking ? 'text-red-700' : 'text-slate-700'"
        >
          Diagnostics
        </h4>
        <p
          v-if="diagnostics.error"
          data-testid="plan-diagnostics-error"
          role="alert"
          class="text-sm text-red-700"
        >
          {{ diagnostics.error }}
        </p>
        <template v-else-if="diagnostics.report">
          <p
            v-if="blocking"
            data-testid="plan-diagnostics-blocking"
            class="text-sm font-medium text-red-700"
          >
            This graph is blocked: it cannot become executable yet.
          </p>
          <p
            v-else
            data-testid="plan-diagnostics-clear"
            class="text-sm text-slate-600"
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
              class="text-sm text-red-700"
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
              class="text-sm text-red-700"
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
              class="text-sm text-red-700"
            >
              Profile reference {{ profile.reference }} resolves to no catalogue entry.
            </li>
          </ul>
        </template>
      </section>

      <div class="grid gap-4 md:grid-cols-2">
        <section class="flex flex-col gap-2">
          <h4 class="text-sm font-semibold text-slate-700">
            Display order
          </h4>
          <ol
            data-testid="plan-specs"
            class="flex flex-col divide-y divide-slate-200 rounded border border-slate-200"
          >
            <li
              v-for="(spec, position) in displayed.spec_numbers"
              :key="spec"
              :data-testid="`plan-spec-row-${spec}`"
              class="flex items-center gap-2 px-3 py-2 text-sm"
            >
              <span class="font-mono">{{ specId(spec) }}</span>
              <span class="ml-auto flex items-center gap-1">
                <button
                  :data-testid="`plan-spec-up-${spec}`"
                  type="button"
                  :disabled="!editable || position === 0"
                  class="rounded border border-slate-300 px-2 py-0.5 text-xs disabled:opacity-30"
                  @click="moveSpec(spec, position - 1)"
                >
                  ↑
                </button>
                <button
                  :data-testid="`plan-spec-down-${spec}`"
                  type="button"
                  :disabled="!editable || position === displayed.spec_numbers.length - 1"
                  class="rounded border border-slate-300 px-2 py-0.5 text-xs disabled:opacity-30"
                  @click="moveSpec(spec, position + 1)"
                >
                  ↓
                </button>
                <button
                  :data-testid="`plan-spec-remove-${spec}`"
                  type="button"
                  :disabled="!editable"
                  class="rounded border border-slate-300 px-2 py-0.5 text-xs disabled:opacity-30"
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
              class="w-52 rounded border border-slate-300 px-3 py-2 text-sm"
            >
            <button
              type="submit"
              data-testid="plan-spec-add"
              :disabled="!editable"
              class="rounded border border-slate-300 px-3 py-2 text-sm hover:bg-slate-50 disabled:opacity-30"
            >
              Add Spec
            </button>
          </form>
        </section>

        <section class="flex flex-col gap-2">
          <h4 class="text-sm font-semibold text-slate-700">
            Dependency edges
          </h4>
          <ul
            data-testid="plan-edges"
            class="flex flex-col divide-y divide-slate-200 rounded border border-slate-200"
          >
            <li
              v-for="edge in displayed.edges"
              :key="`${edge.from_spec}-${edge.to_spec}`"
              :data-testid="`plan-edge-row-${edge.from_spec}-${edge.to_spec}`"
              class="flex items-center gap-2 px-3 py-2 text-sm"
            >
              <span class="font-mono">
                {{ specId(edge.from_spec) }} → {{ specId(edge.to_spec) }}
              </span>
              <button
                :data-testid="`plan-edge-remove-${edge.from_spec}-${edge.to_spec}`"
                type="button"
                :disabled="!editable"
                class="ml-auto rounded border border-slate-300 px-2 py-0.5 text-xs disabled:opacity-30"
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
              class="rounded border border-slate-300 px-2 py-2 text-sm"
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
              class="rounded border border-slate-300 px-2 py-2 text-sm"
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
            <button
              type="submit"
              data-testid="plan-edge-add"
              :disabled="!editable"
              class="rounded border border-slate-300 px-3 py-2 text-sm hover:bg-slate-50 disabled:opacity-30"
            >
              Add edge
            </button>
          </form>
        </section>
      </div>
    </section>
  </main>
</template>
