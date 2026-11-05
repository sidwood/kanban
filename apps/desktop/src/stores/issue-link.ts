// Issue links on the board. The shell announces the key a
// `kanban://issue/KEY` link names, already parsed and checked; the
// core resolves it, and the board route naming the issue's Project
// and Ticket opens its drawer through the board's own link, as a
// palette hit or an Attention Item does. A link waits for the Project
// register first: the drawer finds the issue through its Project, so
// a drawer opened before the register answers would show the Ticket
// without its issue. Only the newest link ever navigates, and a key
// the core cannot resolve, or a register it cannot read, is reported
// as the board's error until the operator moves on.
import { defineStore } from 'pinia'
import { watch } from 'vue'
import type { Router } from 'vue-router'
import { KanbanClient } from '@kanban/contracts'
import type { IssueRecord } from '@kanban/contracts'
import { asApiError } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import { useProjectRegisterStore } from './project-register'
import { boardRouteFor } from './scope'

/** The routes that show a board, where a refused link is reported. */
const BOARD_ROUTES: readonly string[] = ['board', 'global-board']

/** Resolve once `settled` holds. */
function until(settled: () => boolean): Promise<void> {
  if (settled()) return Promise.resolve()
  return new Promise((resolve) => {
    const stop = watch(settled, (done) => {
      if (!done) return
      stop()
      resolve()
    })
  })
}

/** The Project register once it has answered: a read in flight is
 * waited for, and a register never read, or whose last read failed,
 * is read now. Resolves to the read's refusal, if any. */
async function readRegister(transport: ShellTransport): Promise<string | null> {
  const projects = useProjectRegisterStore()
  if (!projects.loading && (!projects.loaded || projects.error !== null)) {
    void projects.refresh(transport)
  }
  await until(() => !projects.loading)
  return projects.error
}

export const useIssueLinkStore = defineStore('issue-link', {
  state: () => ({
    /** The newest key the shell announced that has not been opened:
     * it waits for the core to be reachable. */
    pending: null as string | null,
    /** Why the last link could not be opened. */
    error: null as string | null,
    /** The route that refusal was reported on; leaving it lets the
     * refusal go. */
    errorRoute: null as string | null,
    // The links received so far, so only the newest one navigates or
    // reports.
    issued: 0,
  }),
  actions: {
    /** Hold one key the shell announced until it can be opened; a
     * newer key replaces one still waiting, and an older link still
     * waiting on the core opens and reports nothing from now on. */
    receive(identifier: string): void {
      this.pending = identifier
      this.issued += 1
    },
    /** Open the waiting key once the Project register has answered:
     * its issue's board with the drawer open, or the core's refusal
     * as the board's error. */
    async openPending(transport: ShellTransport, router: Router): Promise<void> {
      const identifier = this.pending
      if (identifier === null) return
      this.pending = null
      this.issued += 1
      const attempt = this.issued
      this.error = null
      this.errorRoute = null
      const report = async (message: string): Promise<void> => {
        if (!BOARD_ROUTES.includes(String(router.currentRoute.value.name))) {
          await router.push('/board')
        }
        if (attempt !== this.issued) return
        this.error = message
        this.errorRoute = router.currentRoute.value.fullPath
      }
      const registerRefusal = await readRegister(transport)
      if (attempt !== this.issued) return
      if (registerRefusal !== null) {
        await report(registerRefusal)
        return
      }
      let issue: IssueRecord
      try {
        issue = await new KanbanClient(transport).queryIssueGet({ identifier })
      } catch (failure) {
        if (attempt !== this.issued) return
        await report(asApiError(failure).message)
        return
      }
      if (attempt !== this.issued) return
      await router.push({
        path: boardRouteFor(issue.project_id),
        query: { ticket: String(issue.ticket_id) },
      })
    },
    /** The operator arrived somewhere: a refusal reported on another
     * route is no longer theirs to read. */
    follow(fullPath: string): void {
      if (this.errorRoute === null || this.errorRoute === fullPath) return
      this.error = null
      this.errorRoute = null
    },
  },
})
