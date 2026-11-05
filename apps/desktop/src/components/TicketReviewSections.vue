<script setup lang="ts">
import type { ReviewSlotRecord } from '@kanban/contracts'
import { useTicketDetailStore } from '../stores/ticket-detail'
import type { DetailSection } from '../stores/ticket-detail'

const detail = useTicketDetailStore()

// A section whose authority did not answer knows nothing about its
// records, which is not the same as knowing there are none.
const UNREADABLE = 'This could not be read; what the core holds is unknown.'

/** Whether a section may state that the core holds no records. */
function absent(section: DetailSection): boolean {
  return detail.readable(section)
}

function slotName(slot: ReviewSlotRecord): string {
  return slot.occupant.kind === 'human' ? 'Human' : slot.occupant.name
}

function slotVerdict(slot: ReviewSlotRecord): string {
  if (!slot.verdict) return 'no verdict yet'
  return `${slot.verdict.approve ? 'approved' : 'rejected'} at ${slot.verdict.tip} — ${slot.verdict.summary}`
}
</script>

<template>
  <div class="flex flex-col gap-6">
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
  </div>
</template>
