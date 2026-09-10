<script setup lang="ts">
// Code, repository, and Seed Workspace are immutable anchors; they are never
// copied into this settings command.
import { computed, inject, reactive, ref, watch } from 'vue'
import { useRoute } from 'vue-router'
import { kanbanTransportKey } from '../core/transport'
import {
  adoptScope,
  emptyScope,
  issueCommand,
  projectScopeKey,
  scopeHolds,
} from '../core/scope-authority'
import { useInitiativesStore } from '../stores/initiatives'
import { useProjectRegisterStore } from '../stores/project-register'
import AppButton from '../components/AppButton.vue'
import InlineAlert from '../components/InlineAlert.vue'
import SectionHeader from '../components/SectionHeader.vue'

const transport = inject(kanbanTransportKey)
const route = useRoute()
const projects = useProjectRegisterStore()
const initiatives = useInitiativesStore()

const draft = reactive({
  name: '',
  default_branch: '',
  herdr_workspace: '',
  herdr_session: '',
  initiative_id: null as number | null,
})
const draftProjectId = ref<number | null>(null)
const draftVersion = ref<number | null>(null)
const draftDirty = ref(false)
const saved = ref(false)
const refusal = ref<string | null>(null)
let takingDraft = false

watch(
  draft,
  () => {
    if (!takingDraft && draftProjectId.value === projectId.value) draftDirty.value = true
  },
  { deep: true, flush: 'sync' },
)

const scope = emptyScope()

const projectId = computed(() => Number(route.params.projectId))

const project = computed(
  () => projects.projects.find((entry) => entry.id === projectId.value) ?? null,
)

watch(project, (current) => {
  if (
    current
    && current.id === draftProjectId.value
    && current.version !== draftVersion.value
    && !draftDirty.value
  ) {
    takeDraft(current.id)
  }
})

const editable = computed(
  () => project.value !== null
    && !project.value.archived
    && draftProjectId.value === projectId.value
    && draftVersion.value !== null,
)

// Vue Router reuses this component when only the Project parameter changes.
watch(
  projectId,
  () => {
    void adoptProject()
  },
  { immediate: true },
)

async function adoptProject(): Promise<void> {
  const target = projectId.value
  const claim = adoptScope(scope, projectScopeKey(target))
  // Invalidate mismatched draft authority before either refresh can yield.
  takeDraft(target)
  saved.value = false
  refusal.value = null
  if (!transport) return
  await Promise.all([projects.refresh(transport), initiatives.refresh(transport)])
  if (!scopeHolds(scope, claim)) return
  if (draftProjectId.value !== target || !draftDirty.value) takeDraft(target)
}

function takeDraft(target: number): void {
  const held = projects.projects.find((entry) => entry.id === target) ?? null
  takingDraft = true
  draftProjectId.value = held ? target : null
  draftVersion.value = held?.version ?? null
  draft.name = held?.name ?? ''
  draft.default_branch = held?.default_branch ?? ''
  draft.herdr_workspace = held?.herdr_workspace ?? ''
  draft.herdr_session = held?.herdr_session ?? ''
  draft.initiative_id = held?.initiative_id ?? null
  draftDirty.value = false
  takingDraft = false
}

async function submit(): Promise<void> {
  const target = projectId.value
  const optimisticVersion = draftVersion.value
  if (!transport || !editable.value || optimisticVersion === null) return
  const claim = issueCommand(scope)
  const outcome = await projects.update(transport, target, optimisticVersion, { ...draft })
  if (!scopeHolds(scope, claim)) return
  saved.value = outcome.landed
  refusal.value = outcome.refusal
  if (outcome.landed) takeDraft(target)
}
</script>

<template>
  <main class="animate-rise flex flex-col gap-6 px-6 py-8 lg:px-8">
    <SectionHeader
      eyebrow="Authoring"
      :title="project ? `${project.code} settings` : 'Project settings'"
      summary="The settings a Project carries after registration. Its code, target repository, and Seed Workspace are not settings: they anchor everything already recorded against it."
    >
      <template #actions>
        <RouterLink
          to="/register"
          data-testid="settings-back"
          class="inline-flex h-8 items-center rounded-control border border-line-strong px-3 text-xs font-medium text-ink transition-colors hover:border-accent/40 hover:bg-accent/8"
        >
          All Projects
        </RouterLink>
      </template>
    </SectionHeader>

    <InlineAlert
      v-if="projects.loaded && !project"
      data-testid="settings-missing"
    >
      Project {{ projectId }} is not registered.
    </InlineAlert>

    <InlineAlert
      v-if="refusal ?? projects.error"
      data-testid="settings-error"
    >
      {{ refusal ?? projects.error }}
    </InlineAlert>

    <section
      v-if="project"
      data-testid="settings-facts"
      class="flex flex-col gap-3 rounded-panel border border-line bg-surface p-4"
    >
      <h2 class="font-display text-lg font-semibold tracking-tight text-ink">
        Anchored facts
      </h2>
      <dl class="grid gap-3 sm:grid-cols-3">
        <div class="flex min-w-0 flex-col gap-1">
          <dt class="text-xs tracking-[0.12em] text-ink-subtle uppercase">
            Code
          </dt>
          <dd
            data-testid="settings-code"
            class="font-mono text-sm text-ink"
          >
            {{ project.code }}
          </dd>
        </div>
        <div class="flex min-w-0 flex-col gap-1">
          <dt class="text-xs tracking-[0.12em] text-ink-subtle uppercase">
            Target repository
          </dt>
          <dd
            data-testid="settings-repository"
            class="font-mono text-sm break-words text-ink"
          >
            {{ project.repository }}
          </dd>
        </div>
        <div class="flex min-w-0 flex-col gap-1">
          <dt class="text-xs tracking-[0.12em] text-ink-subtle uppercase">
            Seed Workspace
          </dt>
          <dd
            data-testid="settings-seed"
            class="font-mono text-sm break-words text-ink"
          >
            {{ project.seed_workspace }}
          </dd>
        </div>
      </dl>
      <p
        data-testid="settings-counters"
        class="font-mono text-xs text-ink-subtle"
      >
        P{{ project.counters.plan }} · S{{ project.counters.spec }} ·
        T{{ project.counters.ticket }}
      </p>
    </section>

    <InlineAlert
      v-if="project?.archived"
      data-testid="settings-archived"
      tone="neutral"
    >
      {{ project.code }} is archived. Archiving is terminal, so its settings no longer change;
      every recorded fact stays exactly as it was.
    </InlineAlert>

    <form
      v-else-if="editable"
      data-testid="settings-form"
      class="flex flex-col gap-4 rounded-panel border border-line bg-surface p-4"
      @submit.prevent="submit"
    >
      <h2 class="font-display text-lg font-semibold tracking-tight text-ink">
        Settings
      </h2>
      <div class="grid gap-3 sm:grid-cols-2">
        <label class="flex min-w-0 flex-col gap-1 text-sm text-ink-muted">
          <span>Name</span>
          <input
            v-model="draft.name"
            data-testid="settings-name"
            aria-label="Project name"
            class="min-w-0 rounded-control border border-line bg-surface px-3 py-2 text-sm text-ink"
          >
        </label>
        <label class="flex min-w-0 flex-col gap-1 text-sm text-ink-muted">
          <span>Default branch</span>
          <input
            v-model="draft.default_branch"
            data-testid="settings-branch"
            aria-label="Default branch"
            class="min-w-0 rounded-control border border-line bg-surface px-3 py-2 text-sm text-ink"
          >
        </label>
        <label class="flex min-w-0 flex-col gap-1 text-sm text-ink-muted">
          <span>Target Herdr workspace</span>
          <input
            v-model="draft.herdr_workspace"
            data-testid="settings-workspace"
            aria-label="Target Herdr workspace"
            class="min-w-0 rounded-control border border-line bg-surface px-3 py-2 text-sm text-ink"
          >
        </label>
        <label class="flex min-w-0 flex-col gap-1 text-sm text-ink-muted">
          <span>Herdr session</span>
          <input
            v-model="draft.herdr_session"
            data-testid="settings-session"
            aria-label="Herdr session name, optional"
            placeholder="Empty uses Herdr's default session"
            class="min-w-0 rounded-control border border-line bg-surface px-3 py-2 text-sm text-ink"
          >
        </label>
        <label class="flex min-w-0 flex-col gap-1 text-sm text-ink-muted">
          <span>Initiative</span>
          <select
            v-model="draft.initiative_id"
            data-testid="settings-initiative"
            aria-label="Initiative"
            class="min-w-0 max-w-full rounded-control border border-line bg-surface px-3 py-2 text-sm text-ink"
          >
            <option :value="null">
              No Initiative
            </option>
            <option
              v-for="entry in initiatives.initiatives"
              :key="entry.id"
              :value="entry.id"
            >
              {{ entry.name }}
            </option>
          </select>
        </label>
      </div>
      <div class="flex flex-wrap items-center gap-3">
        <AppButton
          type="submit"
          data-testid="settings-save"
          size="sm"
          variant="primary"
        >
          Save settings
        </AppButton>
        <p
          v-if="saved && !refusal && project"
          data-testid="settings-saved"
          class="text-sm text-accent"
          role="status"
        >
          {{ project.code }} settings saved at version {{ project.version }}.
        </p>
      </div>
    </form>
  </main>
</template>
