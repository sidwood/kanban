<script setup lang="ts">
// The drawer's actions: the named human commands DR-LC-09 makes
// commands rather than drags — Edit, Park and Unpark, a human review
// decision, and the one audited emergency recovery — plus a Task's
// legal moves as the core judges them. Every one runs its real
// command against the version the record was read at and reports the
// core's refusal; recovery runs only after a confirmation carrying
// the operator and the reason its audit row records, and cancelling
// runs nothing at all (KAN-S4-US6, KAN-S10-US6).
import { computed, inject, ref, watch } from 'vue'
import type { FindingSeverity, ReviewFindingRecord, TicketRecord, TicketState } from '@kanban/contracts'
import { kanbanTransportKey } from '../core/transport'
import { useLifecycleActionsStore } from '../stores/lifecycle-actions'
import { useTicketDetailStore } from '../stores/ticket-detail'
import { useTicketDialogStore } from '../stores/ticket-dialog'
import AppButton from './AppButton.vue'
import InlineAlert from './InlineAlert.vue'

const { ticket, legalTargets, stateLabels } = defineProps<{
  ticket: TicketRecord
  /** Where a human drag may take this Ticket now, as the core
   * judges it: empty for the agent-owned kinds. */
  legalTargets: readonly TicketState[]
  stateLabels: Record<TicketState, string>
}>()

const emit = defineEmits<{
  acted: [record: TicketRecord]
  /** A human verdict landed: the audit the core appended is new. */
  reviewed: []
}>()

const transport = inject(kanbanTransportKey)
const lifecycle = useLifecycleActionsStore()
const detail = useTicketDetailStore()
const dialog = useTicketDialogStore()

const actionError = ref<string | null>(null)
const reviewOpen = ref(false)
const reviewSummary = ref('')
const reviewApprove = ref(true)
// A rejection resolves on a structured finding, so the decision
// carries the finding's own fields rather than a verdict alone; a
// severity is chosen, never defaulted (DR-EP-19).
const findingSeverity = ref<FindingSeverity | ''>('')
const findingInScope = ref(true)
const findingSummary = ref('')
const findingEvidence = ref('')
const findingLocation = ref('')
const findingResolution = ref('')
const recoveryOpen = ref(false)
const recoveryTo = ref<TicketState | ''>('')
const recoveryWho = ref('')
const recoveryWhy = ref('')

// Only a Task answers a human drag; an Implementation or Bug moves
// under its agents (DR-LC-07, DR-LC-08).
const agentOwned = computed(() => ticket.kind !== 'task')

// The human slot the core is waiting on in the stage it is resolving
// now; a slot in a later stage is not open to a verdict and the core
// refuses one.
const waitingSlot = computed(() => detail.waitingHumanSlots[0] ?? null)

const SEVERITIES: FindingSeverity[] = ['p0', 'p1', 'p2', 'p3']

// A rejection that resolves the stage must carry at least one
// blocking in-scope P0 to P2 finding, and every narrative field of
// that finding must say something; the core refuses anything less, so
// the decision has no shape without them (kanban-domain/src/finding.rs).
const reviewGap = computed(() => {
  if (reviewSummary.value.trim() === '') return 'a summary'
  if (reviewApprove.value) return null
  if (findingSeverity.value === '') return 'a finding severity'
  if (!findingInScope.value || findingSeverity.value === 'p3') {
    return 'an in-scope P0 to P2 finding; a rejection cannot resolve on anything less'
  }
  if (findingSummary.value.trim() === '') return 'a finding summary'
  if (findingEvidence.value.trim() === '') return 'the evidence for the finding'
  if (findingLocation.value.trim() === '') return 'the location of the finding'
  if (findingResolution.value.trim() === '') return 'a proposed resolution'
  return null
})

function clearReview(): void {
  reviewOpen.value = false
  reviewSummary.value = ''
  reviewApprove.value = true
  findingSeverity.value = ''
  findingInScope.value = true
  findingSummary.value = ''
  findingEvidence.value = ''
  findingLocation.value = ''
  findingResolution.value = ''
}

// Recovery names a state, an operator, and a reason; the typed
// command has no shape without all three (DR-LC-10).
const recoveryGap = computed(() => {
  if (recoveryTo.value === '') return 'a state'
  if (recoveryWho.value.trim() === '') return 'an operator'
  if (recoveryWhy.value.trim() === '') return 'a reason'
  return null
})

// Recovery may name any state the lifecycle vocabulary holds except
// the one the Ticket already stands in; which of them the Ticket may
// actually reach is the core's judgement, and a refusal is reported
// (DR-LC-10).
const recoveryTargets = computed(() =>
  (Object.keys(stateLabels) as TicketState[]).filter((state) => state !== ticket.state),
)

// Every action guards on the record the drawer is showing.
watch(
  () => ticket,
  (record) => {
    lifecycle.adopt(record)
    actionError.value = null
    clearReview()
    recoveryOpen.value = false
    recoveryTo.value = ''
    recoveryWho.value = ''
    recoveryWhy.value = ''
  },
  { immediate: true },
)

async function run(command: () => Promise<boolean>): Promise<boolean> {
  actionError.value = null
  const landed = await command()
  if (!landed) {
    actionError.value = lifecycle.error
    return false
  }
  if (lifecycle.ticket) emit('acted', lifecycle.ticket)
  return true
}

function edit(): void {
  dialog.openEdit({ projectId: ticket.project_id, ticketId: ticket.id, kind: ticket.kind })
}

async function park(): Promise<void> {
  if (transport) await run(() => lifecycle.park(transport))
}

async function unpark(): Promise<void> {
  if (transport) await run(() => lifecycle.unpark(transport))
}

async function move(event: Event): Promise<void> {
  const target = (event.target as HTMLSelectElement).value as TicketState | ''
  if (!transport || target === '') return
  await run(() => lifecycle.transition(transport, target))
}

async function submitReview(): Promise<void> {
  const slot = waitingSlot.value
  if (!transport || !slot || reviewGap.value !== null) return
  actionError.value = null
  const findings: ReviewFindingRecord[] =
    reviewApprove.value || findingSeverity.value === ''
      ? []
      : [
          {
            severity: findingSeverity.value,
            in_scope: findingInScope.value,
            summary: findingSummary.value,
            evidence: findingEvidence.value,
            location: findingLocation.value,
            proposed_resolution: findingResolution.value,
          },
        ]
  const landed = await detail.submitHumanVerdict(
    transport,
    slot.id,
    reviewApprove.value,
    reviewSummary.value,
    findings,
  )
  if (!landed) {
    actionError.value = detail.error
    return
  }
  clearReview()
  // The verdict appended an audit row against the Ticket; the
  // timeline reads again rather than standing on its earlier answer.
  emit('reviewed')
}

async function confirmRecovery(): Promise<void> {
  if (!transport || recoveryGap.value !== null || recoveryTo.value === '') return
  const landed = await run(() =>
    lifecycle.override(transport, recoveryTo.value as TicketState, recoveryWho.value, recoveryWhy.value),
  )
  if (!landed) return
  recoveryOpen.value = false
  recoveryTo.value = ''
  recoveryWho.value = ''
  recoveryWhy.value = ''
}

function cancelRecovery(): void {
  recoveryOpen.value = false
  recoveryTo.value = ''
  recoveryWho.value = ''
  recoveryWhy.value = ''
}

const FIELD_CLASS =
  'rounded-control border border-line bg-surface px-3 py-2 text-sm text-ink placeholder:text-ink-subtle'
</script>

<template>
  <div class="flex w-full flex-col gap-3">
    <InlineAlert
      v-if="actionError"
      data-testid="drawer-action-error"
    >
      {{ actionError }}
    </InlineAlert>

    <div class="flex flex-wrap items-center justify-end gap-2">
      <p
        v-if="agentOwned"
        data-testid="drawer-agent-owned"
        class="mr-auto text-xs text-ink-subtle"
      >
        This kind's lifecycle is agent-owned; a human moves it only through these commands.
      </p>
      <label
        v-else
        class="mr-auto flex items-center gap-2 text-xs text-ink-muted"
      >
        Move to
        <select
          data-testid="drawer-move"
          aria-label="Move this Task"
          :class="FIELD_CLASS"
          :value="''"
          @change="move"
        >
          <option value="">
            Pick a state
          </option>
          <option
            v-for="target in legalTargets"
            :key="target"
            :value="target"
          >
            {{ stateLabels[target] }}
          </option>
        </select>
      </label>

      <AppButton
        size="sm"
        data-testid="drawer-edit"
        @click="edit"
      >
        Edit
      </AppButton>
      <AppButton
        v-if="ticket.state === 'parked'"
        size="sm"
        data-testid="drawer-unpark"
        @click="unpark"
      >
        Unpark
      </AppButton>
      <AppButton
        v-else
        size="sm"
        data-testid="drawer-park"
        @click="park"
      >
        Park
      </AppButton>
      <AppButton
        v-if="waitingSlot"
        size="sm"
        data-testid="drawer-review-decision"
        @click="reviewOpen = true"
      >
        Record review decision
      </AppButton>
      <AppButton
        variant="dangerQuiet"
        size="sm"
        data-testid="drawer-recover"
        @click="recoveryOpen = true"
      >
        Emergency recovery
      </AppButton>
    </div>

    <form
      v-if="reviewOpen && waitingSlot"
      data-testid="review-decision"
      class="flex flex-col gap-2 rounded-control border border-line bg-surface/70 p-3"
      @submit.prevent="submitReview"
    >
      <p class="text-xs text-ink-subtle">
        Human slot {{ waitingSlot.id }} · reviewing revision {{ detail.review?.tip }}
      </p>
      <label class="flex flex-col gap-1 text-sm text-ink-muted">
        Summary
        <textarea
          v-model="reviewSummary"
          data-testid="review-decision-summary"
          aria-label="Review summary"
          rows="2"
          required
          :class="FIELD_CLASS"
        />
      </label>
      <div
        role="group"
        aria-label="Verdict"
        class="flex gap-2"
      >
        <AppButton
          size="sm"
          data-testid="review-decision-approve"
          :aria-pressed="reviewApprove"
          @click="reviewApprove = true"
        >
          Approve
        </AppButton>
        <AppButton
          size="sm"
          data-testid="review-decision-reject"
          :aria-pressed="!reviewApprove"
          @click="reviewApprove = false"
        >
          Reject
        </AppButton>
      </div>
      <template v-if="!reviewApprove">
        <p class="text-xs text-ink-subtle">
          A rejection stands on a finding, so record the one it resolves on.
        </p>
        <div class="flex flex-wrap items-center gap-3">
          <label class="flex items-center gap-2 text-sm text-ink-muted">
            Severity
            <select
              v-model="findingSeverity"
              data-testid="review-finding-severity"
              aria-label="Finding severity"
              :class="FIELD_CLASS"
            >
              <option value="">
                Pick a severity
              </option>
              <option
                v-for="severity in SEVERITIES"
                :key="severity"
                :value="severity"
              >
                {{ severity.toUpperCase() }}
              </option>
            </select>
          </label>
          <label class="flex items-center gap-2 text-sm text-ink-muted">
            <input
              v-model="findingInScope"
              type="checkbox"
              data-testid="review-finding-in-scope"
              aria-label="In scope"
            >
            In scope
          </label>
        </div>
        <label class="flex flex-col gap-1 text-sm text-ink-muted">
          Finding
          <input
            v-model="findingSummary"
            data-testid="review-finding-summary"
            aria-label="Finding summary"
            :class="FIELD_CLASS"
          >
        </label>
        <label class="flex flex-col gap-1 text-sm text-ink-muted">
          Evidence
          <textarea
            v-model="findingEvidence"
            data-testid="review-finding-evidence"
            aria-label="Finding evidence"
            rows="2"
            :class="FIELD_CLASS"
          />
        </label>
        <label class="flex flex-col gap-1 text-sm text-ink-muted">
          Location
          <input
            v-model="findingLocation"
            data-testid="review-finding-location"
            aria-label="Finding location"
            :class="FIELD_CLASS"
          >
        </label>
        <label class="flex flex-col gap-1 text-sm text-ink-muted">
          Proposed resolution
          <textarea
            v-model="findingResolution"
            data-testid="review-finding-resolution"
            aria-label="Proposed resolution"
            rows="2"
            :class="FIELD_CLASS"
          />
        </label>
      </template>
      <p
        v-if="reviewGap"
        data-testid="review-decision-incomplete"
        class="text-sm text-caution"
      >
        This decision needs {{ reviewGap }} before it can be recorded.
      </p>
      <div class="flex gap-2">
        <AppButton
          variant="primary"
          size="sm"
          type="submit"
          data-testid="review-decision-submit"
          :disabled="reviewGap !== null"
        >
          Record decision
        </AppButton>
        <AppButton
          size="sm"
          data-testid="review-decision-cancel"
          @click="clearReview"
        >
          Cancel
        </AppButton>
      </div>
    </form>

    <form
      v-if="recoveryOpen"
      data-testid="recovery-confirm"
      class="flex flex-col gap-2 rounded-control border border-critical/35 bg-critical/8 p-3"
      @submit.prevent="confirmRecovery"
    >
      <p class="text-sm text-ink">
        Emergency recovery moves this Ticket past the rules and records who did it and why.
        Confirm the move before it runs.
      </p>
      <label class="flex flex-col gap-1 text-sm text-ink-muted">
        Recover to
        <select
          v-model="recoveryTo"
          data-testid="recovery-to"
          aria-label="Recover to"
          :class="FIELD_CLASS"
        >
          <option value="">
            Pick a state
          </option>
          <option
            v-for="target in recoveryTargets"
            :key="target"
            :value="target"
          >
            {{ stateLabels[target] }}
          </option>
        </select>
      </label>
      <label class="flex flex-col gap-1 text-sm text-ink-muted">
        Operator
        <input
          v-model="recoveryWho"
          data-testid="recovery-who"
          aria-label="Operator"
          required
          :class="FIELD_CLASS"
        >
      </label>
      <label class="flex flex-col gap-1 text-sm text-ink-muted">
        Reason
        <textarea
          v-model="recoveryWhy"
          data-testid="recovery-why"
          aria-label="Reason"
          rows="2"
          required
          :class="FIELD_CLASS"
        />
      </label>
      <p
        v-if="recoveryGap"
        data-testid="recovery-incomplete"
        class="text-sm text-caution"
      >
        Recovery needs {{ recoveryGap }} before it can run.
      </p>
      <div class="flex gap-2">
        <AppButton
          variant="danger"
          size="sm"
          type="submit"
          data-testid="recovery-submit"
          :disabled="recoveryGap !== null"
        >
          Confirm recovery
        </AppButton>
        <AppButton
          size="sm"
          data-testid="recovery-cancel"
          @click="cancelRecovery"
        >
          Cancel
        </AppButton>
      </div>
    </form>
  </div>
</template>
