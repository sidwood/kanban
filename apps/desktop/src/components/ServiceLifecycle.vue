<script setup lang="ts">
import { inject, onMounted, onUnmounted } from 'vue'
import { kanbanTransportKey, tauriTransport } from '../core/transport'
import { useServiceLifecycleStore } from '../stores/service-lifecycle'

const transport = inject(kanbanTransportKey, tauriTransport)
const lifecycle = useServiceLifecycleStore()
const unbind = lifecycle.bind(transport)
onMounted(() => lifecycle.refreshLogin(transport))
onUnmounted(unbind)
</script>

<template>
  <section
    aria-labelledby="service-lifecycle-heading"
    class="flex flex-col gap-3 rounded border border-slate-300 p-4"
  >
    <h2
      id="service-lifecycle-heading"
      class="text-xl font-semibold"
    >
      Background service
    </h2>
    <p>Closing Kanban leaves the service running. Only an explicit stop interrupts background capabilities.</p>
    <p data-testid="login-state">
      Launch at login: {{ lifecycle.login?.enabled === true ? 'Enabled' : lifecycle.login?.enabled === false ? 'Disabled' : 'Unknown' }}
    </p>
    <div class="flex gap-3">
      <button
        data-testid="login-enable"
        :disabled="lifecycle.busy || !lifecycle.login"
        @click="lifecycle.setLogin(transport, true)"
      >
        Enable launch at login
      </button>
      <button
        data-testid="login-disable"
        :disabled="lifecycle.busy || !lifecycle.login"
        @click="lifecycle.setLogin(transport, false)"
      >
        Remove launch at login
      </button>
      <button
        :disabled="lifecycle.busy || !lifecycle.connected"
        @click="lifecycle.refreshLogin(transport)"
      >
        Refresh registration
      </button>
    </div>
    <p>Removing launch at login does not stop a running service.</p>
    <button
      data-testid="review-stop"
      :disabled="lifecycle.busy || !lifecycle.connected"
      @click="lifecycle.reviewStop(transport)"
    >
      Review service stop
    </button>
    <div
      v-if="lifecycle.reviewing"
      role="alertdialog"
      aria-labelledby="stop-title"
      aria-describedby="stop-description"
      class="flex flex-col gap-3 border border-red-700 p-4"
      @keydown.esc="lifecycle.cancelStop()"
    >
      <h3
        id="stop-title"
        class="font-semibold"
      >
        Stop the background service?
      </h3>
      <p id="stop-description">
        The Core reports these capabilities will stop. Herdr agents are not terminated and no workflow verdict is inferred.
      </p>
      <ul
        v-if="lifecycle.warning"
        class="list-inside list-disc"
      >
        <li
          v-for="capability in lifecycle.warning.capabilities"
          :key="capability"
        >
          {{ capability }}
        </li>
      </ul>
      <p v-else>
        Loading a current capability warning…
      </p>
      <div class="flex gap-3">
        <button
          data-testid="cancel-stop"
          :disabled="lifecycle.busy"
          @click="lifecycle.cancelStop()"
        >
          Cancel
        </button>
        <button
          data-testid="confirm-stop"
          :disabled="!lifecycle.warning || lifecycle.busy"
          @click="lifecycle.confirmStop(transport)"
        >
          Confirm service stop
        </button>
      </div>
    </div>
    <p
      v-if="lifecycle.message"
      role="status"
    >
      {{ lifecycle.message }}
    </p>
    <p
      v-if="lifecycle.error"
      role="alert"
    >
      {{ lifecycle.error }}
    </p>
  </section>
</template>
