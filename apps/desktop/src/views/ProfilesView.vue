<script setup lang="ts">
// The Execution Profile catalogue, composed into the shell
// (KAN-T140-AC4): the named entries with harness, model, effort, usage
// pool, and fallback policy, defined, updated, and retired through the
// commands (KAN-S7-US1, DR-EP-01, DR-EP-02). Each entry states two
// separate truths about its fallback — the walk the catalogue plans
// now, and the walk the core's own Runs recorded — because a catalogue
// change never rewrites a Run's snapshot (DR-EP-05). Ordinary
// implementer assignment lives here too, and needs no review
// configuration to come first (KAN-T140-AC5, DR-EP-03).
import { computed, inject, onMounted, reactive, ref } from 'vue'
import { KanbanClient } from '@kanban/contracts'
import type { RunRecord } from '@kanban/contracts'
import { asApiError, kanbanTransportKey } from '../core/transport'
import { adoptScope, emptyScope, projectScopeKey, scopeHolds } from '../core/scope-authority'
import { useProfilesStore } from '../stores/profiles'
import { useProjectRegisterStore } from '../stores/project-register'
import { useImplementerAssignmentStore } from '../stores/implementer-assignment'
import { effectiveFallback, plannedFallback } from '../stores/profile-fallback'
import AppButton from '../components/AppButton.vue'
import EmptyState from '../components/EmptyState.vue'
import InlineAlert from '../components/InlineAlert.vue'
import SectionHeader from '../components/SectionHeader.vue'
import StatusBadge from '../components/StatusBadge.vue'

const transport = inject(kanbanTransportKey)
const profiles = useProfilesStore()
const projects = useProjectRegisterStore()
const assignment = useImplementerAssignmentStore()

const blankDraft = () => ({
  name: '',
  harness: '',
  model: '',
  effort: '',
  usage_pool: '',
  fallback: '',
})

const draft = reactive(blankDraft())
const pickedProjectId = ref<number | null>(null)
const pickedTicketId = ref<string>('')
const pickedProfile = ref<string>('')
const runs = ref<RunRecord[]>([])
const runsError = ref<string | null>(null)

// Every Project-scoped load carries the scope it was issued in: the
// Runs and Tickets of a Project the operator has left must never be
// shown, or assigned, under the one they are in (KAN-T140-AC4,
// KAN-T140-AC5, KAN-T145).
const scope = emptyScope()

onMounted(async () => {
  if (!transport) return
  await Promise.all([profiles.refresh(transport), projects.refresh(transport)])
  const first = projects.projects.find((entry) => !entry.archived) ?? projects.projects[0]
  if (first) {
    pickedProjectId.value = first.id
    await openProject(first.id)
  }
})

// The Tickets an assignment can name, and the Runs that say what the
// fallback policy actually did in this Project. The Project being
// left takes its Tickets, its Runs, and the pick made against them.
async function openProject(projectId: number): Promise<void> {
  if (!transport) return
  const claim = adoptScope(scope, projectScopeKey(projectId))
  pickedTicketId.value = ''
  runs.value = []
  runsError.value = null
  await assignment.load(transport, projectId)
  if (!scopeHolds(scope, claim)) return
  try {
    const response = await new KanbanClient(transport).queryRunList({ project_id: projectId })
    if (!scopeHolds(scope, claim)) return
    runs.value = response.runs
    runsError.value = null
  } catch (failure) {
    if (!scopeHolds(scope, claim)) return
    runs.value = []
    runsError.value = asApiError(failure).message
  }
}

const projectCode = computed(
  () => projects.projects.find((entry) => entry.id === pickedProjectId.value)?.code ?? '',
)

// The entries still offered for assignment: a retired entry is
// preserved and listed, and assignable to nothing (DR-EP-03).
const assignable = computed(() => profiles.profiles.filter((entry) => !entry.retired))

function ticketLabel(ticket: { number: number; slice?: string | null; title?: string | null }): string {
  const summary = ticket.slice ?? ticket.title ?? 'Untitled Ticket'
  return `${projectCode.value}-T${ticket.number} — ${summary}`
}

// The walk the core would take for one entry, stated exactly as
// `resolve_effective` takes it: retired hops are crossed, and the
// first entry the catalogue still assigns answers and ends the walk.
// An entry that answers never has its own successor read, so this
// never shows one as part of the path or reports it as a refusal. A
// retired entry is assignable to nothing directly (DR-EP-03); that is
// the picker's restriction, not the walk's.
function planned(name: string): string {
  const walk = plannedFallback(profiles.profiles, name)
  if (walk.chain.length === 0) {
    return `The catalogue holds no entry named ${name}.`
  }
  const sentences: string[] = []
  const answered = walk.chain.join(' → ')
  if (walk.broken === null) {
    const policy = profiles.profiles.find((held) => held.name === walk.effective)?.fallback?.trim()
    sentences.push(`${answered}.`)
    if (walk.chain.length === 1 && !policy) {
      sentences.push('No fallback policy.')
    } else if (policy) {
      sentences.push(
        `Its policy names ${policy}, which the core reads only once ${walk.effective} is retired.`,
      )
    }
  } else if (walk.broken.reason === 'exhausted') {
    sentences.push(
      `${answered}. The walk stops at ${walk.broken.name}, which is retired and names no fallback.`,
    )
  } else {
    const reason = walk.broken.reason === 'unknown'
      ? 'names no catalogue entry'
      : 'is already on this walk'
    sentences.push(`${answered} → ${walk.broken.name}.`)
    sentences.push(`The walk stops at ${walk.broken.name}, which ${reason}.`)
  }
  const retired = walk.chain.filter(
    (entry) => profiles.profiles.find((held) => held.name === entry)?.retired,
  )
  if (retired.length > 0) {
    const plural = retired.length > 1
    const them = plural ? 'them' : 'it'
    // Only an entry the walk went past was crossed; one it stopped at
    // ended the walk instead.
    const crossed = walk.effective === null
      ? `nothing assigns to ${them} directly`
      : `the walk crosses ${them}, and nothing assigns to ${them} directly`
    sentences.push(`${retired.join(', ')} ${plural ? 'are' : 'is'} retired: ${crossed}.`)
  }
  if (walk.effective === null) {
    sentences.push(`No entry on this walk is assignable, so the core refuses a run requesting ${name}.`)
  } else if (walk.effective !== name) {
    sentences.push(`A run requesting ${name} runs ${walk.effective}.`)
  }
  return sentences.join(' ')
}

// What the Project's Runs recorded about one entry's fallback.
function effective(name: string): string {
  const observed = effectiveFallback(runs.value, name)
  if (observed.requested === 0) {
    return `No Run in ${projectCode.value} has requested this profile.`
  }
  if (observed.fellBack === 0) {
    return `${observed.requested} Run${observed.requested === 1 ? '' : 's'} requested it; none fell back.`
  }
  return `${observed.fellBack} of ${observed.requested} fell back · ${observed.paths
    .map((path) => path.join(' → '))
    .join('; ')}`
}

// The assignment one Ticket carries now, as the core holds it.
function assignedProfile(ticket: { profile?: string | null }): string {
  return ticket.profile ?? 'unassigned'
}

async function define() {
  if (transport) {
    const landed = await profiles.define(transport, { ...draft })
    if (landed) {
      Object.assign(draft, blankDraft())
    }
  }
}

async function update(profileIndex: number) {
  if (transport) {
    await profiles.update(transport, profiles.profiles[profileIndex])
  }
}

async function retire(profileIndex: number) {
  if (transport) {
    await profiles.retire(transport, profiles.profiles[profileIndex])
  }
}

async function submitAssignment() {
  const ticketId = Number(pickedTicketId.value)
  if (!transport || !Number.isInteger(ticketId) || ticketId <= 0 || !pickedProfile.value) {
    return
  }
  await assignment.assign(transport, ticketId, pickedProfile.value)
}

async function pickProject(event: Event) {
  const value = Number((event.target as HTMLSelectElement).value)
  pickedProjectId.value = value
  await openProject(value)
}
</script>

<template>
  <main class="animate-rise flex flex-col gap-6 px-6 py-8 lg:px-8">
    <SectionHeader
      eyebrow="Execution"
      title="Execution profiles"
      summary="The catalogue assignments name. Each entry states the fallback its policy plans and the fallback its Runs actually took; the two are separate truths and a catalogue change never rewrites a Run."
    />

    <InlineAlert
      v-if="profiles.error"
      data-testid="profiles-error"
    >
      {{ profiles.error }}
    </InlineAlert>

    <section
      data-testid="profile-assignment"
      class="flex flex-col gap-4 rounded-panel border border-line bg-surface p-4"
    >
      <div class="flex flex-col gap-1">
        <h2 class="font-display text-lg font-semibold tracking-tight text-ink">
          Assign an implementer
        </h2>
        <p class="text-sm text-ink-muted">
          An assignment names one catalogue entry. It stands on its own: a Ticket's review
          configuration is a separate decision and need not exist first.
        </p>
      </div>

      <InlineAlert
        v-if="assignment.error"
        data-testid="assign-error"
      >
        {{ assignment.error }}
      </InlineAlert>
      <InlineAlert
        v-if="runsError"
        data-testid="runs-error"
      >
        {{ runsError }}
      </InlineAlert>

      <form
        class="flex flex-wrap items-end gap-3"
        @submit.prevent="submitAssignment"
      >
        <label class="flex flex-col gap-1 text-sm text-ink-muted">
          Project
          <select
            :value="pickedProjectId ?? ''"
            data-testid="assign-project"
            aria-label="Project"
            class="min-w-0 max-w-full rounded-control border border-line bg-surface px-3 py-2 text-sm text-ink"
            @change="pickProject"
          >
            <option
              v-for="entry in projects.projects"
              :key="entry.id"
              :value="entry.id"
            >
              {{ entry.code }} — {{ entry.name }}
            </option>
          </select>
        </label>
        <label class="flex min-w-0 flex-1 basis-64 flex-col gap-1 text-sm text-ink-muted">
          Ticket
          <select
            v-model="pickedTicketId"
            data-testid="assign-ticket"
            aria-label="Ticket"
            class="min-w-0 max-w-full rounded-control border border-line bg-surface px-3 py-2 text-sm text-ink"
          >
            <option value="">
              Pick a Ticket
            </option>
            <option
              v-for="entry in assignment.tickets"
              :key="entry.id"
              :value="String(entry.id)"
            >
              {{ ticketLabel(entry) }}
            </option>
          </select>
        </label>
        <label class="flex flex-col gap-1 text-sm text-ink-muted">
          Execution Profile
          <select
            v-model="pickedProfile"
            data-testid="assign-profile"
            aria-label="Execution Profile"
            class="min-w-0 max-w-full rounded-control border border-line bg-surface px-3 py-2 text-sm text-ink"
          >
            <option value="">
              Pick a profile
            </option>
            <option
              v-for="entry in assignable"
              :key="entry.name"
              :value="entry.name"
            >
              {{ entry.name }}
            </option>
          </select>
        </label>
        <AppButton
          type="submit"
          data-testid="assign-submit"
          variant="primary"
          size="sm"
        >
          Assign implementer
        </AppButton>
      </form>

      <ul
        v-if="assignment.tickets.length"
        data-testid="assign-tickets"
        class="flex flex-col divide-y divide-line overflow-hidden rounded-control border border-line"
      >
        <li
          v-for="entry in assignment.tickets"
          :key="entry.id"
          class="flex flex-wrap items-center gap-3 px-3 py-2 text-sm"
        >
          <span class="font-mono text-xs text-ink">{{ projectCode }}-T{{ entry.number }}</span>
          <span class="min-w-0 flex-1 truncate text-ink-muted">
            {{ entry.slice ?? entry.title ?? 'Untitled Ticket' }}
          </span>
          <span
            :data-testid="`assign-ticket-${entry.id}-profile`"
            class="rounded-control bg-rail px-2 py-0.5 font-mono text-xs text-ink-muted"
          >
            {{ assignedProfile(entry) }}
          </span>
        </li>
      </ul>
      <EmptyState
        v-else-if="assignment.loaded"
        compact
        message="This Project holds no Ticket to assign."
      />
    </section>

    <section
      data-testid="profile-define"
      class="flex flex-col gap-4 rounded-panel border border-line bg-surface p-4"
    >
      <h2 class="font-display text-lg font-semibold tracking-tight text-ink">
        Define a profile
      </h2>
      <div class="grid gap-3 sm:grid-cols-3">
        <label class="flex flex-col gap-1 text-sm text-ink-muted">
          <span>Name</span>
          <input
            v-model="draft.name"
            data-testid="define-name"
            class="rounded-control border border-line bg-surface px-2 py-1 text-ink"
          >
        </label>
        <label class="flex flex-col gap-1 text-sm text-ink-muted">
          <span>Harness</span>
          <input
            v-model="draft.harness"
            data-testid="define-harness"
            class="rounded-control border border-line bg-surface px-2 py-1 text-ink"
          >
        </label>
        <label class="flex flex-col gap-1 text-sm text-ink-muted">
          <span>Model family</span>
          <input
            v-model="draft.model"
            data-testid="define-model"
            class="rounded-control border border-line bg-surface px-2 py-1 text-ink"
          >
        </label>
        <label class="flex flex-col gap-1 text-sm text-ink-muted">
          <span>Effort</span>
          <input
            v-model="draft.effort"
            data-testid="define-effort"
            class="rounded-control border border-line bg-surface px-2 py-1 text-ink"
          >
        </label>
        <label class="flex flex-col gap-1 text-sm text-ink-muted">
          <span>Usage pool</span>
          <input
            v-model="draft.usage_pool"
            data-testid="define-usage-pool"
            class="rounded-control border border-line bg-surface px-2 py-1 text-ink"
          >
        </label>
        <label class="flex flex-col gap-1 text-sm text-ink-muted">
          <span>Fallback (optional profile name)</span>
          <input
            v-model="draft.fallback"
            data-testid="define-fallback"
            class="rounded-control border border-line bg-surface px-2 py-1 text-ink"
          >
        </label>
      </div>
      <AppButton
        data-testid="define-submit"
        variant="primary"
        size="sm"
        class="w-fit"
        @click="define"
      >
        Define profile
      </AppButton>
    </section>

    <section
      v-if="profiles.loaded"
      data-testid="profile-list"
      class="flex flex-col gap-4 rounded-panel border border-line bg-surface p-4"
    >
      <h2 class="font-display text-lg font-semibold tracking-tight text-ink">
        Catalogue
      </h2>
      <EmptyState
        v-if="profiles.profiles.length === 0"
        compact
        data-testid="profile-empty"
        message="No profiles defined."
      />
      <ul class="flex list-none flex-col gap-3">
        <li
          v-for="(profile, index) in profiles.profiles"
          :key="profile.name"
          data-testid="profile-row"
          class="flex flex-col gap-3 rounded-control border border-line p-3"
        >
          <div class="flex items-center justify-between gap-3">
            <span
              data-testid="profile-name"
              class="font-mono text-sm font-medium text-ink"
            >
              {{ profile.name }}
            </span>
            <StatusBadge
              v-if="profile.retired"
              data-testid="profile-retired"
              tone="neutral"
              density="compact"
            >
              retired
            </StatusBadge>
          </div>
          <div class="grid gap-2 sm:grid-cols-3">
            <label class="flex flex-col gap-1 text-sm text-ink-muted">
              <span class="text-ink-subtle">Harness</span>
              <input
                v-model="profile.harness"
                :data-testid="`row-harness-${profile.name}`"
                :disabled="profile.retired"
                class="rounded-control border border-line bg-surface px-2 py-1 text-ink disabled:bg-rail"
              >
            </label>
            <label class="flex flex-col gap-1 text-sm text-ink-muted">
              <span class="text-ink-subtle">Model family</span>
              <input
                v-model="profile.model"
                :data-testid="`row-model-${profile.name}`"
                :disabled="profile.retired"
                class="rounded-control border border-line bg-surface px-2 py-1 text-ink disabled:bg-rail"
              >
            </label>
            <label class="flex flex-col gap-1 text-sm text-ink-muted">
              <span class="text-ink-subtle">Effort</span>
              <input
                v-model="profile.effort"
                :data-testid="`row-effort-${profile.name}`"
                :disabled="profile.retired"
                class="rounded-control border border-line bg-surface px-2 py-1 text-ink disabled:bg-rail"
              >
            </label>
            <label class="flex flex-col gap-1 text-sm text-ink-muted">
              <span class="text-ink-subtle">Usage pool</span>
              <input
                v-model="profile.usage_pool"
                :data-testid="`row-usage-pool-${profile.name}`"
                :disabled="profile.retired"
                class="rounded-control border border-line bg-surface px-2 py-1 text-ink disabled:bg-rail"
              >
            </label>
            <label class="flex flex-col gap-1 text-sm text-ink-muted">
              <span class="text-ink-subtle">Fallback</span>
              <input
                v-model="profile.fallback"
                :data-testid="`row-fallback-${profile.name}`"
                :disabled="profile.retired"
                class="rounded-control border border-line bg-surface px-2 py-1 text-ink disabled:bg-rail"
              >
            </label>
          </div>

          <dl class="grid gap-2 text-xs sm:grid-cols-2">
            <div class="flex flex-col gap-0.5">
              <dt class="text-ink-subtle">
                Planned fallback
              </dt>
              <dd
                :data-testid="`profile-planned-${profile.name}`"
                class="font-mono text-ink-muted"
              >
                {{ planned(profile.name) }}
              </dd>
            </div>
            <div class="flex flex-col gap-0.5">
              <dt class="text-ink-subtle">
                Effective fallback in {{ projectCode || 'this Project' }}
              </dt>
              <dd
                :data-testid="`profile-effective-${profile.name}`"
                class="font-mono text-ink-muted"
              >
                {{ effective(profile.name) }}
              </dd>
            </div>
          </dl>

          <div
            v-if="!profile.retired"
            class="flex gap-2"
          >
            <AppButton
              :data-testid="`row-update-${profile.name}`"
              variant="primary"
              size="sm"
              @click="update(index)"
            >
              Save
            </AppButton>
            <AppButton
              :data-testid="`row-retire-${profile.name}`"
              size="sm"
              @click="retire(index)"
            >
              Retire
            </AppButton>
          </div>
        </li>
      </ul>
    </section>
  </main>
</template>
