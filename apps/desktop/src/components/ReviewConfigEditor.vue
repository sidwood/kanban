<script setup lang="ts">
// The staged review editor: pick one Ticket of the picked Project
// and compose its review configuration — ordered stages of parallel
// slots, each slot required or optional and occupied by a human or
// by a named Execution Profile — then replace the whole set in one
// act (KAN-S10-US1, DR-EP-09). Presentation only; separation is the
// core's rule at configuration time, and a refusal is reported
// while the standing configuration survives.
import { computed, inject, ref, watch } from 'vue'
import type { TicketRecord, TicketReviewStage } from '@kanban/contracts'
import { kanbanTransportKey } from '../core/transport'
import { useReviewConfigStore } from '../stores/review-config'

type SlotDraft = {
  occupant: 'human' | 'profile'
  profile: string
  requirement: 'required' | 'optional'
}

type StageDraft = {
  slots: SlotDraft[]
}

const props = defineProps<{
  tickets: TicketRecord[]
  projectCode: string
}>()

const transport = inject(kanbanTransportKey)
const review = useReviewConfigStore()

const pickedTicketId = ref<number | null>(null)
const draft = ref<StageDraft[]>([blankStage()])

function blankSlot(): SlotDraft {
  return { occupant: 'profile', profile: '', requirement: 'required' }
}

function blankStage(): StageDraft {
  return { slots: [blankSlot()] }
}

const pickedTicket = computed(() =>
  props.tickets.find((ticket) => ticket.id === pickedTicketId.value) ?? null,
)

function ticketId(ticket: { number: number }): string {
  return `${props.projectCode}-T${ticket.number}`
}

function summary(ticket: { slice?: string | null; title?: string | null }): string {
  return ticket.slice ?? ticket.title ?? ''
}

// Picking a Ticket loads its standing configuration and seeds the
// editor from it — the blank single-stage draft while none stands.
async function pickTicket(id: number): Promise<void> {
  pickedTicketId.value = id
  if (transport) {
    await review.refresh(transport, id)
    draft.value = review.config ? stagesOf(review.config.stages) : [blankStage()]
  }
}

function stagesOf(stages: TicketReviewStage[]): StageDraft[] {
  return stages.map((stage) => ({
    slots: stage.slots.map((slot) => ({
      occupant: slot.occupant.kind === 'human' ? 'human' : 'profile',
      profile: slot.occupant.kind === 'profile' ? slot.occupant.name : '',
      requirement: slot.requirement,
    })),
  }))
}

function addStage(): void {
  draft.value.push(blankStage())
}

function removeStage(position: number): void {
  draft.value.splice(position, 1)
}

function addSlot(stage: number): void {
  draft.value[stage]?.slots.push(blankSlot())
}

function removeSlot(stage: number, position: number): void {
  draft.value[stage]?.slots.splice(position, 1)
}

// Replace the whole configuration through the generated client; the
// store reports a refusal and keeps what stands.
async function submitConfigure(): Promise<void> {
  if (transport && pickedTicketId.value !== null) {
    const stages: TicketReviewStage[] = draft.value.map((stage) => ({
      slots: stage.slots.map((slot) => ({
        occupant:
          slot.occupant === 'human'
            ? { kind: 'human' as const }
            : { kind: 'profile' as const, name: slot.profile.trim() },
        requirement: slot.requirement,
      })),
    }))
    await review.configure(transport, pickedTicketId.value, stages)
  }
}

// A reloaded configuration re-seeds the editor, so a replaced set
// renders as it landed.
watch(
  () => review.config,
  (standing) => {
    if (pickedTicketId.value !== null) {
      draft.value = standing ? stagesOf(standing.stages) : [blankStage()]
    }
  },
)
</script>

<template>
  <section
    data-testid="review-config-editor"
    class="flex flex-col gap-4 rounded-lg border border-slate-200 p-4"
  >
    <h2 class="text-sm font-semibold text-slate-700">
      Staged review
    </h2>

    <label class="flex w-fit flex-col gap-1 text-sm text-slate-600">
      Ticket
      <select
        :value="pickedTicketId ?? ''"
        data-testid="review-ticket-pick"
        aria-label="Ticket whose review is configured"
        class="rounded border border-slate-300 px-3 py-2 text-sm"
        @change="pickTicket(Number(($event.target as HTMLSelectElement).value))"
      >
        <option
          value=""
          disabled
        >
          Pick a Ticket
        </option>
        <option
          v-for="ticket in tickets"
          :key="ticket.id"
          :value="ticket.id"
        >
          {{ ticketId(ticket) }} — {{ summary(ticket) }}{{ ticket.profile ? ` (implementer: ${ticket.profile})` : '' }}
        </option>
      </select>
    </label>

    <p
      v-if="review.error"
      data-testid="review-config-error"
      role="alert"
      class="rounded border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-700"
    >
      {{ review.error }}
    </p>

    <template v-if="pickedTicket">
      <p
        v-if="review.loaded"
        data-testid="review-config-standing"
        class="text-xs text-slate-500"
      >
        {{ review.config ? `Standing configuration at version ${review.config.version}.` : 'No configuration stands yet.' }}
      </p>

      <form
        class="flex flex-col gap-4"
        @submit.prevent="submitConfigure"
      >
        <fieldset
          v-for="(stage, stagePosition) in draft"
          :key="stagePosition"
          :data-testid="`review-stage-${stagePosition}`"
          class="flex flex-col gap-2"
        >
          <legend class="text-sm font-medium text-slate-600">
            Stage {{ stagePosition + 1 }} — parallel slots
          </legend>
          <div
            v-for="(slot, slotPosition) in stage.slots"
            :key="slotPosition"
            class="flex flex-wrap items-center gap-2"
          >
            <select
              v-model="slot.occupant"
              :data-testid="`review-slot-occupant-${stagePosition}-${slotPosition}`"
              :aria-label="`Stage ${stagePosition + 1} slot ${slotPosition + 1} occupant`"
              class="rounded border border-slate-300 px-2 py-1.5 text-sm"
            >
              <option value="profile">
                profile
              </option>
              <option value="human">
                human
              </option>
            </select>
            <input
              v-if="slot.occupant === 'profile'"
              v-model="slot.profile"
              :data-testid="`review-slot-profile-${stagePosition}-${slotPosition}`"
              :aria-label="`Stage ${stagePosition + 1} slot ${slotPosition + 1} profile`"
              placeholder="Execution Profile name"
              class="min-w-44 flex-1 rounded border border-slate-300 px-3 py-2 text-sm"
            >
            <select
              v-model="slot.requirement"
              :data-testid="`review-slot-requirement-${stagePosition}-${slotPosition}`"
              :aria-label="`Stage ${stagePosition + 1} slot ${slotPosition + 1} requirement`"
              class="rounded border border-slate-300 px-2 py-1.5 text-sm"
            >
              <option value="required">
                required
              </option>
              <option value="optional">
                optional
              </option>
            </select>
            <button
              :data-testid="`review-slot-remove-${stagePosition}-${slotPosition}`"
              type="button"
              class="rounded border border-slate-300 px-2 py-1 text-xs hover:bg-slate-50"
              @click="removeSlot(stagePosition, slotPosition)"
            >
              Remove slot
            </button>
          </div>
          <div class="flex flex-wrap gap-2">
            <button
              :data-testid="`review-slot-add-${stagePosition}`"
              type="button"
              class="w-fit rounded border border-slate-300 px-2 py-1 text-xs hover:bg-slate-50"
              @click="addSlot(stagePosition)"
            >
              Add slot
            </button>
            <button
              :data-testid="`review-stage-remove-${stagePosition}`"
              type="button"
              class="w-fit rounded border border-slate-300 px-2 py-1 text-xs hover:bg-slate-50"
              @click="removeStage(stagePosition)"
            >
              Remove stage
            </button>
          </div>
        </fieldset>

        <button
          data-testid="review-stage-add"
          type="button"
          class="w-fit rounded border border-slate-300 px-3 py-1.5 text-sm hover:bg-slate-50"
          @click="addStage"
        >
          Add stage
        </button>

        <button
          type="submit"
          data-testid="review-config-configure"
          class="w-fit rounded bg-slate-900 px-3 py-2 text-sm font-medium text-white hover:bg-slate-700"
        >
          Configure staged review
        </button>
      </form>
    </template>
  </section>
</template>
