// Issue links on the board. The shell announces the key a
// `kanban://issue/KEY` link names, already parsed and checked; the
// core resolves it, and the board route naming the issue's Project
// and Ticket opens its drawer through the board's own link, as a
// palette hit or an Attention Item does. Only the newest link ever
// navigates, and a key the core cannot resolve is reported as the
// board's error until the operator moves on.
import { defineStore } from 'pinia'
import type { Router } from 'vue-router'
import { KanbanClient } from '@kanban/contracts'
import type { IssueRecord } from '@kanban/contracts'
import { asApiError } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import { boardRouteFor } from './scope'

/** The routes that show a board, where a refused link is reported. */
const BOARD_ROUTES: readonly string[] = ['board', 'global-board']

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
    // The links taken up so far, so only the newest one navigates or
    // reports.
    issued: 0,
  }),
  actions: {
    /** Hold one key the shell announced until it can be opened; a
     * newer key replaces one still waiting. */
    receive(identifier: string): void {
      this.pending = identifier
    },
    /** Open the waiting key: its issue's board with the drawer open,
     * or the core's refusal as the board's error. */
    async openPending(transport: ShellTransport, router: Router): Promise<void> {
      const identifier = this.pending
      if (identifier === null) return
      this.pending = null
      this.issued += 1
      const attempt = this.issued
      this.error = null
      this.errorRoute = null
      let issue: IssueRecord
      try {
        issue = await new KanbanClient(transport).queryIssueGet({ identifier })
      } catch (failure) {
        if (attempt !== this.issued) return
        const message = asApiError(failure).message
        if (!BOARD_ROUTES.includes(String(router.currentRoute.value.name))) {
          await router.push('/board')
        }
        if (attempt !== this.issued) return
        this.error = message
        this.errorRoute = router.currentRoute.value.fullPath
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
