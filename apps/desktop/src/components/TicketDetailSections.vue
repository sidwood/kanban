<script setup lang="ts">
// The drawer's authoritative detail for one Ticket: the revision each
// criterion's evidence is bound at, what holds the Ticket back, the
// execution it is under, the review stages and their verdicts, the
// findings recorded against it, and the evidence attached to it
// (KAN-S10-US5, KAN-S10-US6). Every line comes from a record the core
// holds; where it holds none, the section says so.
import { computed } from 'vue'
import type { ReviewSlotRecord, TicketRecord } from '@kanban/contracts'
import { useTicketDetailStore } from '../stores/ticket-detail'
import type { DetailSection } from '../stores/ticket-detail'
import InlineAlert from './InlineAlert.vue'

const { ticket } = defineProps<{
  ticket: TicketRecord
}>()

const detail = useTicketDetailStore()

// A section whose authority did not answer knows nothing about its
// records, which is not the same as knowing there are none.
const UNREADABLE = 'This could not be read; what the core holds is unknown.'

/** Whether a section may state that the core holds no records. */
function absent(section: DetailSection): boolean {
  return detail.readable(section)
}

/** How one criterion's bound evidence stands, in the core's own
 * words; a criterion nothing is bound to says exactly that. */
function revisionOf(index: number): string {
  if (!absent('criteria')) return UNREADABLE
  const binding = detail.bindingFor(index)
  if (!binding) return 'No evidence bound yet'
  const marks = [
    `revision ${binding.tip}`,
    `evidence ${binding.evidence_id}`,
    binding.review,
    binding.satisfied ? 'satisfied' : 'not satisfied',
  ]
  if (binding.void) marks.push('void')
  return marks.join(' · ')
}

/** A Ticket's registered identity: the Project-scoped number is only
 * unique within its Project, so a blocking Ticket is named with the
 * code of the Project that owns it, whichever Project that is
 * (crates/kanban-dto/src/ticket.rs). */
function dependencyIdentity(projectId: number, number: number): string {
  return `${detail.projectCode(projectId) ?? '?'}-T${number}`
}

function slotName(slot: ReviewSlotRecord): string {
  return slot.occupant.kind === 'human' ? 'Human' : slot.occupant.name
}

function slotVerdict(slot: ReviewSlotRecord): string {
  if (!slot.verdict) return 'no verdict yet'
  return `${slot.verdict.approve ? 'approved' : 'rejected'} at ${slot.verdict.tip} — ${slot.verdict.summary}`
}

const execution = computed(() => {
  const lines: string[] = []
  if (!absent('execution')) return [UNREADABLE]
  const run = detail.currentRun
  if (run) {
    lines.push(`Run ${run.id} · ${run.status}`)
    lines.push(`Planned profile ${run.requested.name}`)
    lines.push(
      run.fallback
        ? `Effective profile ${run.effective.name} (fallback via ${run.fallback_path.join(' → ')})`
        : `Effective profile ${run.effective.name}`,
    )
  } else if (detail.attempts.length > 0) {
    lines.push(`No run is executing this Ticket now; ${detail.attempts.length} have.`)
  } else {
    lines.push('No run has executed this Ticket.')
  }
  if (!absent('placement')) {
    lines.push('The Lane and Workspace could not be read.')
    return lines
  }
  const lane = detail.lane
  if (lane) lines.push(`Lane ${lane.id}`)
  const workspace = detail.workspace
  if (workspace) lines.push(`Workspace ${workspace.path} · ${workspace.health}`)
  return lines
})
</script>

<template>
  <div class="flex flex-col gap-6">
    <InlineAlert
      v-if="detail.error"
      data-testid="drawer-detail-error"
    >
      {{ detail.error }}
    </InlineAlert>

    <section
      v-if="ticket.criteria.length > 0"
      class="flex flex-col gap-2"
      data-testid="drawer-criteria"
    >
      <h3 class="font-display text-sm font-semibold tracking-tight text-ink">
        Story-linked criteria
      </h3>
      <ul class="flex flex-col gap-2">
        <li
          v-for="(criterion, position) in ticket.criteria"
          :key="position"
          class="rounded-control border border-line bg-surface/70 px-3 py-2 text-sm text-ink"
        >
          <p>{{ criterion.outcome }}</p>
          <p
            :data-testid="`drawer-criterion-revision-${position}`"
            class="mt-1 font-mono text-xs text-ink-subtle"
          >
            {{ revisionOf(position) }}
          </p>
        </li>
      </ul>
    </section>

    <section
      class="flex flex-col gap-2"
      data-testid="drawer-dependencies"
    >
      <h3 class="font-display text-sm font-semibold tracking-tight text-ink">
        Dependencies
      </h3>
      <ul
        v-if="detail.dependencies.length > 0 || detail.blockers.length > 0"
        class="flex flex-col gap-2"
      >
        <li
          v-for="dependency in detail.dependencies"
          :key="`dependency-${dependency.from_ticket_id}`"
          :data-testid="`drawer-dependency-${dependency.from_ticket_id}`"
          class="rounded-control border border-line bg-surface/70 px-3 py-2 text-sm text-ink"
        >
          Waits on
          <span class="font-mono">{{
            dependencyIdentity(dependency.from_project_id, dependency.from_number)
          }}</span>
          · {{ dependency.from_state }}
        </li>
        <li
          v-for="blocker in detail.blockers"
          :key="`blocker-${blocker.id}`"
          :data-testid="`drawer-blocker-${blocker.id}`"
          class="rounded-control border border-caution/35 bg-caution/8 px-3 py-2 text-sm text-ink"
        >
          External blocker — {{ blocker.description }}
        </li>
      </ul>
      <p
        v-else-if="absent('dependencies')"
        class="text-sm text-ink-subtle"
      >
        Nothing holds this Ticket back.
      </p>
      <p
        v-else
        data-testid="drawer-dependencies-unreadable"
        class="text-sm text-caution"
      >
        {{ UNREADABLE }}
      </p>
    </section>

    <section
      class="flex flex-col gap-2"
      data-testid="drawer-execution"
    >
      <h3 class="font-display text-sm font-semibold tracking-tight text-ink">
        Execution
      </h3>
      <p
        v-for="line in execution"
        :key="line"
        class="text-sm text-ink-muted"
      >
        {{ line }}
      </p>
    </section>

    <section
      class="flex flex-col gap-2"
      data-testid="drawer-reviews"
    >
      <h3 class="font-display text-sm font-semibold tracking-tight text-ink">
        Reviews
      </h3>
      <p
        v-if="detail.needsRevalidation"
        data-testid="drawer-review-revalidation"
        class="rounded-control border border-caution/35 bg-caution/8 px-3 py-2 text-sm text-ink"
      >
        This Ticket needs revalidation before any approval can stand again.
      </p>
      <template v-if="detail.review">
        <div
          v-for="stage in detail.review.stages"
          :key="stage.index"
          :data-testid="`drawer-review-stage-${stage.index}`"
          class="flex flex-col gap-1"
        >
          <p class="text-xs font-semibold tracking-[0.06em] text-ink-subtle uppercase">
            Stage {{ stage.index + 1 }} · {{ stage.status }}
          </p>
          <p
            v-for="slot in stage.slots"
            :key="slot.id"
            :data-testid="`drawer-review-slot-${slot.id}`"
            class="rounded-control border border-line bg-surface/70 px-3 py-2 text-sm text-ink"
          >
            {{ slotName(slot) }} · {{ slot.requirement }} · {{ slotVerdict(slot) }}
          </p>
        </div>
      </template>
      <ul
        v-if="detail.reviewAttempts.length > 0"
        class="flex flex-col gap-1"
      >
        <li
          v-for="entry in detail.reviewAttempts"
          :key="entry.attempt"
          :data-testid="`drawer-review-attempt-${entry.attempt}`"
          class="text-xs text-ink-subtle"
        >
          Attempt {{ entry.attempt }} · {{ entry.outcome
          }}<template v-if="entry.verdicts.length">
            · {{ entry.verdicts.join(', ') }}
          </template><template v-if="entry.invalidations.length">
            · invalidated: {{ entry.invalidations.join(', ') }}
          </template>
        </li>
      </ul>
      <p
        v-if="!absent('review')"
        data-testid="drawer-reviews-unreadable"
        class="text-sm text-caution"
      >
        {{ UNREADABLE }}
      </p>
      <p
        v-else-if="!detail.review && detail.reviewAttempts.length === 0"
        class="text-sm text-ink-subtle"
      >
        No review has run on this Ticket.
      </p>
    </section>

    <section
      class="flex flex-col gap-2"
      data-testid="drawer-findings"
    >
      <h3 class="font-display text-sm font-semibold tracking-tight text-ink">
        Findings
      </h3>
      <ul
        v-if="detail.findings.length > 0"
        class="flex flex-col gap-2"
      >
        <li
          v-for="record in detail.findings"
          :key="record.id"
          :data-testid="`drawer-finding-${record.id}`"
          class="flex flex-col gap-0.5 rounded-control border border-line bg-surface/70 px-3 py-2 text-sm text-ink"
        >
          <p>
            <span class="font-mono text-xs uppercase">{{ record.finding.severity }}</span>
            · {{ record.finding.in_scope ? 'in scope' : 'out of scope' }}
            · {{ record.blocking ? 'blocking' : 'not blocking' }}
          </p>
          <p>{{ record.finding.summary }}</p>
          <p class="font-mono text-xs text-ink-subtle">
            {{ record.finding.location }} · revision {{ record.tip }}
          </p>
          <p class="text-xs text-ink-muted">
            Evidence — {{ record.finding.evidence }}
          </p>
          <p class="text-xs text-ink-muted">
            Proposed — {{ record.finding.proposed_resolution }}
          </p>
        </li>
      </ul>
      <p
        v-else-if="absent('findings')"
        class="text-sm text-ink-subtle"
      >
        No findings recorded against this Ticket.
      </p>
      <p
        v-else
        data-testid="drawer-findings-unreadable"
        class="text-sm text-caution"
      >
        {{ UNREADABLE }}
      </p>
    </section>

    <section
      class="flex flex-col gap-2"
      data-testid="drawer-evidence"
    >
      <h3 class="font-display text-sm font-semibold tracking-tight text-ink">
        Evidence
      </h3>
      <ul
        v-if="detail.evidence.length > 0"
        class="flex flex-col gap-2"
      >
        <li
          v-for="record in detail.evidence"
          :key="record.id"
          :data-testid="`drawer-evidence-${record.id}`"
          class="rounded-control border border-line bg-surface/70 px-3 py-2 text-sm text-ink"
        >
          <span class="font-mono text-xs">{{ record.evidence_kind }}</span>
          <template v-if="record.commit_identity">
            · {{ record.commit_identity }}
          </template>
          <template v-if="record.relative_path">
            · {{ record.relative_path }}
          </template>
        </li>
      </ul>
      <p
        v-else-if="absent('evidence')"
        class="text-sm text-ink-subtle"
      >
        No evidence attached to this Ticket.
      </p>
      <p
        v-else
        data-testid="drawer-evidence-unreadable"
        class="text-sm text-caution"
      >
        {{ UNREADABLE }}
      </p>
    </section>
  </div>
</template>
