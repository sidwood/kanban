<script setup lang="ts">
// Quick Bug capture: the small dialog a global shortcut opens, asking
// for the three facts a capture is made of — title, actual behaviour,
// and reporter evidence — and nothing a qualification owns
// (DR-TK-08, KAN-S4-US3). Capture sends `ticket.create`; the core
// judges the facts and a refusal is reported with what was typed
// still in the form.
import { computed, inject, ref, watch } from 'vue'
import { kanbanTransportKey } from '../core/transport'
import { useProjectRegisterStore } from '../stores/project-register'
import { useTicketDialogStore } from '../stores/ticket-dialog'
import { blankTicketDraft, useTicketEditorStore } from '../stores/ticket-editor'
import AppButton from './AppButton.vue'
import AppDialog from './AppDialog.vue'
import InlineAlert from './InlineAlert.vue'

const transport = inject(kanbanTransportKey)
const dialog = useTicketDialogStore()
const projects = useProjectRegisterStore()
const editor = useTicketEditorStore()

const projectId = ref<number | null>(null)
const captureError = ref<string | null>(null)
const title = ref('')
const actualBehaviour = ref('')
const reporterEvidence = ref('')

// The three facts a capture cannot be built without: the typed
// request has no shape while one is blank (DR-TK-08).
const captureGap = computed(() => {
  if (projectId.value === null) return 'a Project'
  if (title.value.trim() === '') return 'a title'
  if (actualBehaviour.value.trim() === '') return 'the actual behaviour'
  if (reporterEvidence.value.trim() === '') return 'the reporter evidence'
  return null
})

watch(
  () => dialog.quickBugOpen,
  (open) => {
    if (!open) return
    captureError.value = null
    projectId.value = dialog.quickBugProjectId
    void adopt()
  },
  { immediate: true },
)

async function adopt(): Promise<void> {
  if (!transport) return
  if (!projects.loaded) await projects.refresh(transport)
  if (projectId.value === null) {
    const first = projects.projects.find((entry) => !entry.archived) ?? projects.projects[0]
    projectId.value = first?.id ?? null
  }
}

async function capture(): Promise<void> {
  if (!transport || captureGap.value !== null || projectId.value === null) return
  const draft = blankTicketDraft()
  draft.kind = 'bug'
  draft.title = title.value
  draft.actualBehaviour = actualBehaviour.value
  draft.reporterEvidence = reporterEvidence.value
  captureError.value = null
  const landed = await editor.create(transport, projectId.value, draft)
  if (!landed) {
    captureError.value = editor.error
    return
  }
  title.value = ''
  actualBehaviour.value = ''
  reporterEvidence.value = ''
  dialog.closeQuickBug()
}

const FIELD_CLASS =
  'rounded-control border border-line bg-surface px-3 py-2 text-sm text-ink placeholder:text-ink-subtle'
</script>

<template>
  <AppDialog
    :open="dialog.quickBugOpen"
    title="Capture a Bug"
    size="small"
    testid="quick-bug-dialog"
    @close="dialog.closeQuickBug()"
  >
    <template #subtitle>
      Qualification comes later; this records the defect while it is fresh.
    </template>

    <InlineAlert
      v-if="captureError"
      data-testid="quick-bug-error"
    >
      {{ captureError }}
    </InlineAlert>

    <form
      class="flex flex-col gap-3"
      @submit.prevent="capture"
    >
      <label
        v-if="projects.projects.length > 1"
        class="flex flex-col gap-1 text-sm text-ink-muted"
      >
        Project
        <select
          v-model="projectId"
          data-testid="quick-bug-project"
          aria-label="Project"
          :class="FIELD_CLASS"
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

      <label class="flex flex-col gap-1 text-sm text-ink-muted">
        Title
        <input
          v-model="title"
          data-testid="quick-bug-title"
          aria-label="Bug title"
          required
          placeholder="What is incorrect?"
          :class="FIELD_CLASS"
        >
      </label>

      <label class="flex flex-col gap-1 text-sm text-ink-muted">
        Actual behaviour — what happened
        <textarea
          v-model="actualBehaviour"
          data-testid="quick-bug-actual"
          aria-label="Actual behaviour"
          rows="2"
          required
          placeholder="What did you see happen?"
          :class="FIELD_CLASS"
        />
      </label>

      <label class="flex flex-col gap-1 text-sm text-ink-muted">
        Reporter evidence — what you hold
        <textarea
          v-model="reporterEvidence"
          data-testid="quick-bug-evidence"
          aria-label="Reporter evidence"
          rows="2"
          required
          placeholder="What evidence do you hold for it?"
          :class="FIELD_CLASS"
        />
      </label>

      <p
        v-if="captureGap"
        data-testid="quick-bug-incomplete"
        class="text-sm text-caution"
      >
        A capture needs {{ captureGap }}.
      </p>

      <AppButton
        variant="primary"
        type="submit"
        class="w-fit"
        data-testid="quick-bug-capture"
        :disabled="captureGap !== null"
      >
        Capture Bug
      </AppButton>
    </form>
  </AppDialog>
</template>
