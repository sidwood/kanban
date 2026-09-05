<script setup lang="ts">
// The Execution Profile catalogue management surface: the named
// entries with harness, model, effort, usage pool, and fallback
// policy, defined, updated, and retired through the commands
// (KAN-S7-US1, DR-EP-01, DR-EP-02). Retired entries stay listed
// with their recorded facts.
import { inject, onMounted, reactive } from 'vue'
import { kanbanTransportKey } from '../core/transport'
import { useProfilesStore } from '../stores/profiles'

const transport = inject(kanbanTransportKey)
const profiles = useProfilesStore()

const blankDraft = () => ({
  name: '',
  harness: '',
  model: '',
  effort: '',
  usage_pool: '',
  fallback: '',
})

const draft = reactive(blankDraft())

onMounted(() => {
  if (transport) {
    void profiles.refresh(transport)
  }
})

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
      <span class="text-slate-900">Execution profiles</span>
    </nav>

    <h1 class="text-3xl font-semibold tracking-tight">
      Execution profiles
    </h1>

    <p
      v-if="profiles.error"
      data-testid="profiles-error"
      role="alert"
      class="rounded border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-700"
    >
      {{ profiles.error }}
    </p>

    <section
      data-testid="profile-define"
      class="rounded border border-slate-200 p-4"
    >
      <h2 class="text-lg font-semibold text-slate-900">
        Define a profile
      </h2>
      <div class="mt-4 grid gap-3 sm:grid-cols-3">
        <label class="flex flex-col gap-1 text-sm">
          <span>Name</span>
          <input
            v-model="draft.name"
            data-testid="define-name"
            class="rounded border border-slate-300 px-2 py-1"
          >
        </label>
        <label class="flex flex-col gap-1 text-sm">
          <span>Harness</span>
          <input
            v-model="draft.harness"
            data-testid="define-harness"
            class="rounded border border-slate-300 px-2 py-1"
          >
        </label>
        <label class="flex flex-col gap-1 text-sm">
          <span>Model family</span>
          <input
            v-model="draft.model"
            data-testid="define-model"
            class="rounded border border-slate-300 px-2 py-1"
          >
        </label>
        <label class="flex flex-col gap-1 text-sm">
          <span>Effort</span>
          <input
            v-model="draft.effort"
            data-testid="define-effort"
            class="rounded border border-slate-300 px-2 py-1"
          >
        </label>
        <label class="flex flex-col gap-1 text-sm">
          <span>Usage pool</span>
          <input
            v-model="draft.usage_pool"
            data-testid="define-usage-pool"
            class="rounded border border-slate-300 px-2 py-1"
          >
        </label>
        <label class="flex flex-col gap-1 text-sm">
          <span>Fallback (optional profile name)</span>
          <input
            v-model="draft.fallback"
            data-testid="define-fallback"
            class="rounded border border-slate-300 px-2 py-1"
          >
        </label>
      </div>
      <button
        type="button"
        data-testid="define-submit"
        class="mt-4 rounded bg-slate-900 px-3 py-2 text-sm text-white"
        @click="define"
      >
        Define profile
      </button>
    </section>

    <section
      v-if="profiles.loaded"
      data-testid="profile-list"
      class="rounded border border-slate-200 p-4"
    >
      <h2 class="text-lg font-semibold text-slate-900">
        Catalogue
      </h2>
      <p
        v-if="profiles.profiles.length === 0"
        data-testid="profile-empty"
        class="mt-3 text-sm text-slate-500"
      >
        No profiles defined.
      </p>
      <ul class="mt-3 flex list-none flex-col gap-3">
        <li
          v-for="(profile, index) in profiles.profiles"
          :key="profile.name"
          data-testid="profile-row"
          class="rounded border border-slate-100 p-3"
        >
          <div class="flex items-center justify-between gap-3">
            <span
              data-testid="profile-name"
              class="font-medium text-slate-900"
            >
              {{ profile.name }}
            </span>
            <span
              v-if="profile.retired"
              data-testid="profile-retired"
              class="rounded bg-slate-100 px-2 py-0.5 text-xs text-slate-600"
            >
              retired
            </span>
          </div>
          <div class="mt-2 grid gap-2 sm:grid-cols-3">
            <label class="flex flex-col gap-1 text-sm">
              <span class="text-slate-500">Harness</span>
              <input
                v-model="profile.harness"
                :data-testid="`row-harness-${profile.name}`"
                :disabled="profile.retired"
                class="rounded border border-slate-300 px-2 py-1 disabled:bg-slate-50"
              >
            </label>
            <label class="flex flex-col gap-1 text-sm">
              <span class="text-slate-500">Model family</span>
              <input
                v-model="profile.model"
                :data-testid="`row-model-${profile.name}`"
                :disabled="profile.retired"
                class="rounded border border-slate-300 px-2 py-1 disabled:bg-slate-50"
              >
            </label>
            <label class="flex flex-col gap-1 text-sm">
              <span class="text-slate-500">Effort</span>
              <input
                v-model="profile.effort"
                :data-testid="`row-effort-${profile.name}`"
                :disabled="profile.retired"
                class="rounded border border-slate-300 px-2 py-1 disabled:bg-slate-50"
              >
            </label>
            <label class="flex flex-col gap-1 text-sm">
              <span class="text-slate-500">Usage pool</span>
              <input
                v-model="profile.usage_pool"
                :data-testid="`row-usage-pool-${profile.name}`"
                :disabled="profile.retired"
                class="rounded border border-slate-300 px-2 py-1 disabled:bg-slate-50"
              >
            </label>
            <label class="flex flex-col gap-1 text-sm">
              <span class="text-slate-500">Fallback</span>
              <input
                v-model="profile.fallback"
                :data-testid="`row-fallback-${profile.name}`"
                :disabled="profile.retired"
                class="rounded border border-slate-300 px-2 py-1 disabled:bg-slate-50"
              >
            </label>
          </div>
          <div
            v-if="!profile.retired"
            class="mt-3 flex gap-2"
          >
            <button
              type="button"
              :data-testid="`row-update-${profile.name}`"
              class="rounded bg-slate-900 px-3 py-1.5 text-sm text-white"
              @click="update(index)"
            >
              Save
            </button>
            <button
              type="button"
              :data-testid="`row-retire-${profile.name}`"
              class="rounded border border-slate-300 px-3 py-1.5 text-sm text-slate-700"
              @click="retire(index)"
            >
              Retire
            </button>
          </div>
        </li>
      </ul>
    </section>
  </main>
</template>
