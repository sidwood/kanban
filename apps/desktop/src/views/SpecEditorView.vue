<script setup lang="ts">
// The Spec editor surface: pick a Project, author Specs, edit the
// nine PRD sections, switch between every content version, diff two
// versions section by section, approve and supersede explicitly, join
// a Plan, and move execution along its closed set — independently of
// content. Presentation only; every domain call goes through the
// generated client in the spec-editor store (KAN-S3-US4, KAN-S3-US5).
import { computed, inject, onMounted, ref, watch } from 'vue'
import { kanbanTransportKey } from '../core/transport'
import { useProjectRegisterStore } from '../stores/project-register'
import { usePlanEditorStore } from '../stores/plan-editor'
import {
  MOVABLE_EXECUTION_STATES,
  SPEC_SECTIONS,
  useSpecEditorStore,
} from '../stores/spec-editor'
import type { SpecContent, SpecExecutionState } from '@kanban/contracts'

const transport = inject(kanbanTransportKey)
const projects = useProjectRegisterStore()
const plans = usePlanEditorStore()
const editor = useSpecEditorStore()

const pickedProjectId = ref<number | null>(null)
const authorName = ref('')
const joinPlanId = ref<number | null>(null)
const moveTarget = ref<SpecExecutionState>('ready')

// The editing form: one field per PRD section, reset whenever the
// displayed version changes so frozen content is never edited in
// place.
const sectionDraft = ref<Record<string, string>>({})

onMounted(() => {
  if (transport) {
    void projects.refresh(transport).then(() => {
      const first = projects.projects.find((project) => !project.archived) ?? projects.projects[0]
      if (first) {
        pickedProjectId.value = first.id
        void loadAll(first.id)
      }
    })
  }
})

// Load the picked Project's Specs and the Plans it offers.
async function loadAll(projectId: number): Promise<void> {
  editor.select(0)
  await Promise.all([
    editor.refresh(transport!, projectId),
    plans.refresh(transport!, projectId),
  ])
}

// The code of the picked Project, for rendering Spec identities.
const projectCode = computed(
  () => projects.projects.find((project) => project.id === pickedProjectId.value)?.code ?? '',
)

function specId(spec: { number: number }): string {
  return `${projectCode.value}-S${spec.number}`
}

// The Spec the editor has open.
const selected = computed(() => editor.selected)

// The version on display and its content.
const displayed = computed(() => editor.displayed)

// Editing is legal only while the working draft is on display;
// approved and superseded versions are immutable.
const editable = computed(
  () => editor.selectedVersion === null && editor.draft !== null,
)

// Reset the section form whenever the displayed version changes.
watch(
  () => displayed.value,
  (now) => {
    const next: Record<string, string> = {}
    for (const section of SPEC_SECTIONS) {
      next[section] = now ? now.content[section] : ''
    }
    sectionDraft.value = next
  },
  { immediate: true },
)

// The version switcher's entries: the working content plus every
// version, newest first, each carrying its state.
const switcher = computed(() => [
  {
    key: 'current' as const,
    number: null as number | null,
    label: 'Current',
    state: editor.currentVersion?.state ?? 'draft',
  },
  ...[...editor.versions].reverse().map((version) => ({
    key: version.number as number | 'current',
    number: version.number as number | null,
    label: `v${version.number}`,
    state: version.state,
  })),
])

// The open Plans of the picked Project the Spec may join.
const openPlansToJoin = computed(() => plans.activeSurface)

const stateLabels: Record<string, string> = {
  draft: 'draft',
  approved: 'approved',
  superseded: 'superseded',
  unplanned: 'unplanned',
  planned: 'planned',
  blocked: 'blocked',
  ready: 'ready',
  active: 'active',
  integration_review: 'integration review',
  complete: 'complete',
  cancelled: 'cancelled',
}

const sectionLabels: Record<string, string> = {
  name: 'Name',
  short_description: 'Short description',
  problem_statement: 'Problem statement',
  solution: 'Solution',
  user_stories: 'User stories',
  implementation_decisions: 'Implementation decisions',
  testing_decisions: 'Testing decisions',
  out_of_scope: 'Out of scope',
  further_notes: 'Further notes',
}

async function submitCreate(): Promise<void> {
  if (transport && pickedProjectId.value !== null) {
    const sections = Object.fromEntries(SPEC_SECTIONS.map((section) => [section, '']))
    sections.name = authorName.value
    await editor.create(transport, pickedProjectId.value, sections as SpecContent)
    authorName.value = ''
  }
}

async function submitContent(): Promise<void> {
  if (!transport || !editable.value) {
    return
  }
  await editor.updateContent(transport, sectionDraft.value as SpecContent)
}

async function submitApprove(): Promise<void> {
  if (transport) {
    await editor.approve(transport)
  }
}

async function submitSupersede(): Promise<void> {
  if (transport && editor.approvedVersion) {
    await editor.supersede(transport, editor.approvedVersion.number)
  }
}

async function submitJoin(): Promise<void> {
  if (transport && joinPlanId.value !== null) {
    await editor.joinPlan(transport, joinPlanId.value)
    joinPlanId.value = null
  }
}

async function submitMove(): Promise<void> {
  if (transport) {
    await editor.moveExecution(transport, moveTarget.value)
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
      <RouterLink
        to="/planning"
        class="hover:text-slate-900"
      >
        Planning
      </RouterLink>
      <span aria-hidden="true"> / </span>
      <span class="text-slate-900">Specs</span>
    </nav>

    <h1 class="text-3xl font-semibold tracking-tight">
      Author Specs
    </h1>

    <div class="flex items-end gap-3">
      <label class="flex flex-col gap-1 text-sm text-slate-600">
        Project
        <select
          v-model="pickedProjectId"
          data-testid="spec-project"
          aria-label="Project"
          class="rounded border border-slate-300 px-3 py-2 text-sm"
          @change="pickedProjectId !== null && loadAll(pickedProjectId)"
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
      <form
        class="flex items-end gap-2"
        @submit.prevent="submitCreate"
      >
        <label class="flex flex-col gap-1 text-sm text-slate-600">
          New Spec name
          <input
            v-model="authorName"
            data-testid="spec-name-input"
            class="rounded border border-slate-300 px-3 py-2 text-sm"
          >
        </label>
        <button
          type="submit"
          data-testid="spec-create"
          class="rounded bg-slate-900 px-3 py-2 text-sm font-medium text-white hover:bg-slate-700"
        >
          Author Spec
        </button>
      </form>
    </div>

    <p
      v-if="editor.error"
      data-testid="spec-error"
      role="alert"
      class="rounded border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-700"
    >
      {{ editor.error }}
    </p>

    <section
      v-if="editor.loaded"
      data-testid="spec-list"
      class="flex flex-col gap-4"
    >
      <ul
        data-testid="spec-index"
        class="flex flex-col divide-y divide-slate-200 rounded border border-slate-200"
      >
        <li
          v-for="spec in editor.specs"
          :key="spec.id"
          :data-testid="`spec-row-${spec.id}`"
          class="flex cursor-pointer items-center gap-3 px-4 py-3 hover:bg-slate-50"
          @click="editor.open(transport!, spec.id)"
        >
          <span class="rounded bg-slate-100 px-2 py-0.5 font-mono text-sm font-medium">
            {{ specId(spec) }}
          </span>
          <span class="text-sm">{{ spec.name }}</span>
          <span
            :data-testid="`spec-execution-${spec.id}`"
            class="rounded bg-slate-100 px-2 py-0.5 text-xs text-slate-600"
          >{{ stateLabels[spec.execution] }}</span>
          <span
            v-if="spec.plan_id !== null"
            class="font-mono text-xs text-slate-500"
          >P{{ spec.plan_id }}</span>
        </li>
      </ul>
    </section>
    <p
      v-else-if="!editor.error"
      data-testid="spec-loading"
    >
      Loading Specs…
    </p>

    <section
      v-if="selected && displayed"
      data-testid="spec-editor"
      class="flex flex-col gap-4 rounded-lg border border-slate-200 p-4"
    >
      <header class="flex flex-wrap items-center gap-3">
        <h3
          data-testid="spec-title"
          class="font-mono text-lg font-semibold"
        >
          {{ specId(selected) }}
        </h3>
        <span
          data-testid="spec-execution"
          class="rounded bg-slate-100 px-2 py-0.5 text-xs text-slate-600"
        >{{ stateLabels[selected.execution] }}</span>
        <span
          data-testid="spec-version-state"
          class="rounded bg-slate-100 px-2 py-0.5 text-xs text-slate-600"
        >{{ stateLabels[displayed.state] }}</span>
        <div class="ml-auto flex flex-wrap items-center gap-1">
          <button
            v-for="entry in switcher"
            :key="entry.key"
            :data-testid="`spec-version-${entry.key}`"
            type="button"
            class="rounded border border-slate-300 px-2 py-1 font-mono text-xs hover:bg-slate-50"
            :class="(editor.selectedVersion ?? null) === entry.number ? 'bg-slate-900 text-white' : ''"
            @click="entry.number === null ? editor.showCurrent() : editor.showVersion(entry.number)"
          >
            {{ entry.label }} · {{ stateLabels[entry.state] }}
          </button>
        </div>
      </header>

      <div class="flex flex-wrap items-center gap-2">
        <form @submit.prevent="submitApprove">
          <button
            type="submit"
            data-testid="spec-approve"
            :disabled="!editor.canApprove"
            class="rounded bg-slate-900 px-3 py-1.5 text-sm font-medium text-white hover:bg-slate-700 disabled:opacity-30"
          >
            Approve draft
          </button>
        </form>
        <form
          v-if="editor.approvedVersion"
          @submit.prevent="submitSupersede"
        >
          <button
            type="submit"
            :data-testid="`spec-supersede-${editor.approvedVersion.number}`"
            class="rounded border border-slate-300 px-3 py-1.5 text-sm hover:bg-slate-50"
          >
            Supersede v{{ editor.approvedVersion.number }}
          </button>
        </form>
        <form
          v-if="selected.execution === 'unplanned' && openPlansToJoin.length"
          class="flex items-center gap-2"
          @submit.prevent="submitJoin"
        >
          <select
            v-model.number="joinPlanId"
            data-testid="spec-plan-select"
            aria-label="Plan to join"
            class="rounded border border-slate-300 px-2 py-1.5 text-sm"
          >
            <option
              v-for="plan in openPlansToJoin"
              :key="plan.id"
              :value="plan.id"
            >
              {{ projectCode }}-P{{ plan.number }}
            </option>
          </select>
          <button
            type="submit"
            data-testid="spec-plan-join"
            class="rounded border border-slate-300 px-3 py-1.5 text-sm hover:bg-slate-50"
          >
            Join Plan
          </button>
        </form>
        <form
          v-if="!['complete', 'cancelled'].includes(selected.execution)"
          class="flex items-center gap-2"
          @submit.prevent="submitMove"
        >
          <select
            v-model="moveTarget"
            data-testid="spec-execution-select"
            aria-label="Execution target"
            class="rounded border border-slate-300 px-2 py-1.5 text-sm"
          >
            <option
              v-for="state in MOVABLE_EXECUTION_STATES"
              :key="state"
              :value="state"
            >
              {{ stateLabels[state] }}
            </option>
          </select>
          <button
            type="submit"
            data-testid="spec-execution-move"
            class="rounded border border-slate-300 px-3 py-1.5 text-sm hover:bg-slate-50"
          >
            Move execution
          </button>
        </form>
      </div>

      <p
        v-if="!editable"
        data-testid="spec-readonly"
        class="text-xs text-slate-500"
      >
        {{ editor.selectedVersion === null
          ? 'Only a draft version accepts content edits; approve or supersede first.'
          : `Viewing v${editor.selectedVersion}; switch to Current to edit a draft.` }}
      </p>

      <form
        class="flex flex-col gap-3"
        @submit.prevent="submitContent"
      >
        <div
          v-for="section in SPEC_SECTIONS"
          :key="section"
          class="flex flex-col gap-1"
        >
          <label
            :for="`spec-section-${section}`"
            class="text-sm font-semibold text-slate-700"
          >
            {{ sectionLabels[section] }}
          </label>
          <textarea
            :id="`spec-section-${section}`"
            :key="`${displayed.number}-${section}`"
            v-model="sectionDraft[section]"
            :data-testid="`spec-section-${section}`"
            :readonly="!editable"
            rows="2"
            class="rounded border border-slate-300 px-3 py-2 text-sm read-only:bg-slate-50"
          />
        </div>
        <div>
          <button
            type="submit"
            data-testid="spec-save"
            :disabled="!editable"
            class="rounded bg-slate-900 px-3 py-1.5 text-sm font-medium text-white hover:bg-slate-700 disabled:opacity-30"
          >
            Save content
          </button>
        </div>
      </form>

      <section
        v-if="editor.diff"
        data-testid="spec-diff"
        class="flex flex-col gap-3 rounded border border-slate-200 bg-slate-50 p-3"
      >
        <h4 class="text-sm font-semibold text-slate-700">
          Diff v{{ editor.comparedVersion }} → v{{ editor.selectedVersion ?? displayed.number }}
        </h4>
        <p
          v-if="editor.diff.every((entry) => !entry.removed.length && !entry.added.length)"
          data-testid="spec-diff-empty"
          class="text-xs text-slate-500"
        >
          No differences.
        </p>
        <div
          v-for="entry in editor.diff.filter((row) => row.removed.length || row.added.length)"
          :key="entry.section"
          :data-testid="`spec-diff-section-${entry.section}`"
          class="flex flex-col gap-1"
        >
          <span class="text-xs font-semibold text-slate-600">{{ sectionLabels[entry.section] }}</span>
          <ul class="flex flex-col gap-0.5 font-mono text-xs">
            <li
              v-for="line in entry.removed"
              :key="`removed-${line}`"
              class="rounded bg-red-50 px-2 py-0.5 text-red-700 line-through"
            >
              − {{ line }}
            </li>
            <li
              v-for="line in entry.added"
              :key="`added-${line}`"
              class="rounded bg-green-50 px-2 py-0.5 text-green-700"
            >
              + {{ line }}
            </li>
          </ul>
        </div>
        <div class="flex flex-wrap items-center gap-1">
          <span class="text-xs text-slate-500">Compare with:</span>
          <button
            v-for="entry in switcher"
            :key="`compare-${entry.key}`"
            :data-testid="`spec-compare-${entry.key}`"
            type="button"
            class="rounded border border-slate-300 px-2 py-1 font-mono text-xs hover:bg-slate-50"
            :class="editor.comparedVersion === entry.number ? 'bg-slate-900 text-white' : ''"
            @click="entry.number !== null && editor.compareWith(entry.number)"
          >
            {{ entry.label }}
          </button>
        </div>
      </section>
      <div
        v-else
        class="flex flex-wrap items-center gap-1"
      >
        <span class="text-xs text-slate-500">Compare with:</span>
        <button
          v-for="entry in switcher"
          :key="`compare-${entry.key}`"
          :data-testid="`spec-compare-${entry.key}`"
          type="button"
          class="rounded border border-slate-300 px-2 py-1 font-mono text-xs hover:bg-slate-50"
          @click="entry.number !== null && editor.compareWith(entry.number)"
        >
          {{ entry.label }}
        </button>
      </div>
    </section>
  </main>
</template>
