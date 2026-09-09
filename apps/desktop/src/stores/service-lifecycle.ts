import { defineStore } from 'pinia'
import { KanbanClient } from '@kanban/contracts'
import type { LoginLaunchState, ServiceStopWarning } from '@kanban/contracts'
import { asApiError } from '../core/transport'
import type { ShellTransport } from '../core/transport'

export const useServiceLifecycleStore = defineStore('service-lifecycle', {
  state: () => ({
    warning: null as ServiceStopWarning | null,
    reviewing: false,
    warningGeneration: 0,
    busy: false,
    login: null as LoginLaunchState | null,
    loginGeneration: 0,
    mutationGeneration: 0,
    bindingGeneration: 0,
    connected: true,
    instanceId: null as string | null,
    error: null as string | null,
    message: null as string | null,
  }),
  actions: {
    bind(transport: ShellTransport) {
      const generation = ++this.bindingGeneration
      this.connected = true
      this.invalidate()
      const unsubscribe = transport.onConnectionChange((state) => {
        if (generation !== this.bindingGeneration) return
        this.connected = state === 'connected'
        this.invalidate()
        if (state === 'connected') void this.refreshLogin(transport)
      })
      return () => {
        unsubscribe()
        if (generation !== this.bindingGeneration) return
        this.bindingGeneration++
        this.connected = false
        this.invalidate()
      }
    },
    invalidate() {
      this.loginGeneration++
      this.mutationGeneration++
      this.warningGeneration++
      this.instanceId = null
      this.login = null
      this.warning = null
      this.reviewing = false
      this.busy = false
      this.error = null
      this.message = null
    },
    observeInstance(instanceId: string) {
      if (this.instanceId !== null && this.instanceId !== instanceId) this.invalidate()
      this.instanceId = instanceId
    },
    async refreshLogin(transport: ShellTransport) {
      if (!this.connected) return
      const generation = ++this.loginGeneration
      try {
        const login = await new KanbanClient(transport).queryServiceLogin_launchGet({})
        if (generation !== this.loginGeneration) return
        this.observeInstance(login.instance_id)
        this.login = login
        this.error = login.error ?? null
      } catch (failure) {
        if (generation !== this.loginGeneration) return
        this.login = null
        this.error = asApiError(failure).message
      }
    },
    async reviewStop(transport: ShellTransport) {
      if (!this.connected) return
      const generation = ++this.warningGeneration
      this.reviewing = true
      this.warning = null
      this.error = null
      try {
        const warning = await new KanbanClient(transport).queryServiceStop_warning({})
        if (generation !== this.warningGeneration) return
        this.observeInstance(warning.instance_id)
        this.warning = warning
        this.reviewing = true
      } catch (failure) {
        if (generation === this.warningGeneration) this.error = asApiError(failure).message
      }
    },
    cancelStop() {
      if (this.busy) return
      this.warningGeneration++
      this.reviewing = false
      this.warning = null
    },
    async confirmStop(transport: ShellTransport) {
      if (!this.warning || this.busy) return
      const warning = this.warning
      this.loginGeneration++
      const generation = ++this.mutationGeneration
      this.busy = true
      try {
        await new KanbanClient(transport).commandServiceStop({
          mutation: { optimistic_version: warning.version, idempotency_key: crypto.randomUUID() },
          instance_id: warning.instance_id,
          warning_id: warning.warning_id,
          confirmed: true,
        })
        if (generation !== this.mutationGeneration) return
        this.message = 'Stop requested. The service may still be shutting down. Reopen Kanban to start it on demand.'
      } catch (failure) {
        if (generation !== this.mutationGeneration) return
        this.error = `Stop could not be confirmed: ${asApiError(failure).message}. Service state is unknown; review a fresh warning before retrying.`
      } finally {
        if (generation === this.mutationGeneration) {
          this.busy = false
          this.cancelStop()
        }
      }
    },
    async setLogin(transport: ShellTransport, enabled: boolean) {
      if (!this.login || this.busy) return
      const login = this.login
      this.loginGeneration++
      const generation = ++this.mutationGeneration
      this.busy = true
      this.error = null
      try {
        await new KanbanClient(transport).commandServiceLogin_launchSet({
          mutation: { optimistic_version: login.version, idempotency_key: crypto.randomUUID() },
          instance_id: login.instance_id,
          enabled,
        })
        if (generation !== this.mutationGeneration) return
        await this.refreshLogin(transport)
      } catch (failure) {
        if (generation !== this.mutationGeneration) return
        this.login = null
        this.error = `Registration could not be confirmed: ${asApiError(failure).message}`
      } finally {
        if (generation === this.mutationGeneration) this.busy = false
      }
    },
  },
})
