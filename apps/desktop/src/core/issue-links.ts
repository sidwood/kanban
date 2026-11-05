// The shell's issue links: a `kanban://issue/KEY` link the operating
// system handed the shell, which the shell has already parsed and
// checked against the form the core mints. The WebView never sees the
// link itself, only the key the shell announces, and holds no
// deep-link capability of its own.
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import type { InjectionKey } from 'vue'

// The shell's names for its announcement and for the call that says
// the WebView is listening. Shell-level plumbing, not domain
// contracts, mirroring the shell crate's `issue_link` module.
const ISSUE_LINK_EVENT = 'shell://issue-link'
const ISSUE_LINK_LISTEN = 'issue_link_listen'

/** One issue a link asks the application to open, mirroring the
 * shell crate's `IssueLinkOpened`. */
export interface IssueLinkOpened {
  /** The issue's key, `CORE-12`. */
  identifier: string
}

/** Where the application hears about issue links. */
export interface IssueLinkSource {
  /** Deliver every issue link until the returned stopper is called,
   * starting with one the shell held because it arrived before
   * anything listened: the link that launched the application. */
  onIssueLink(handler: (link: IssueLinkOpened) => void): () => void
}

function isIssueLinkOpened(payload: unknown): payload is IssueLinkOpened {
  return (
    typeof payload === 'object' &&
    payload !== null &&
    typeof (payload as { identifier?: unknown }).identifier === 'string' &&
    (payload as { identifier: string }).identifier !== ''
  )
}

export const tauriIssueLinks: IssueLinkSource = {
  onIssueLink(handler: (link: IssueLinkOpened) => void): () => void {
    let unlisten: () => void = () => undefined
    let active = true
    listen<unknown>(ISSUE_LINK_EVENT, (entry) => {
      if (isIssueLinkOpened(entry.payload)) handler({ identifier: entry.payload.identifier })
    })
      .then((stop) => {
        if (!active) {
          stop()
          return
        }
        unlisten = stop
        // Only an attached listener can hear the link the shell is
        // holding, so the shell is asked for it now and not before.
        return invoke(ISSUE_LINK_LISTEN)
      })
      .catch(() => undefined)
    return () => {
      active = false
      unlisten()
    }
  },
}

// The injection key the app provides the shell's issue links under.
export const issueLinkSourceKey: InjectionKey<IssueLinkSource> = Symbol('issue-links')
