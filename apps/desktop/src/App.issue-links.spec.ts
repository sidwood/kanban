// Issue links the shell announces open on the board: the link's
// Project and the issue's drawer, whether the link launched the app
// or reached it while it ran, and a key the core cannot find is the
// board's error rather than a crash.
import { flushPromises, mount } from '@vue/test-utils'
import type { VueWrapper } from '@vue/test-utils'
import { createPinia } from 'pinia'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import type { CommentListItem, IssueRecord, TicketRecord } from '@kanban/contracts'
import router from './router'
import { issueLinkSourceKey } from './core/issue-links'
import type { IssueLinkOpened, IssueLinkSource } from './core/issue-links'
import { kanbanTransportKey } from './core/transport'
import type { ShellTransport } from './core/transport'
import { coreProject, edgeProject, harness, ticket } from './test/shell-harness'
import type { HarnessOptions } from './test/shell-harness'
import App from './App.vue'

// CORE-12 on the Control plane board and EDGE-3 on the Edge one.
function tickets(): TicketRecord[] {
  return [
    ticket(),
    ticket({ id: 11, number: 3, project_id: 2, state: 'in_review', title: 'Edge review', version: 1 }),
  ]
}

// The shell's side of the bridge: a link that launched the app is
// held until the WebView listens, then handed over; later links are
// announced as they arrive.
function shellLinks(launchedWith: string | null = null) {
  const handlers: Array<(link: IssueLinkOpened) => void> = []
  let held = launchedWith
  const source: IssueLinkSource = {
    onIssueLink(handler) {
      handlers.push(handler)
      const identifier = held
      held = null
      if (identifier !== null) queueMicrotask(() => handler({ identifier }))
      return () => {
        handlers.splice(handlers.indexOf(handler), 1)
      }
    },
  }
  return {
    source,
    announce(identifier: string) {
      for (const handler of [...handlers]) handler({ identifier })
    },
  }
}

const mounted: VueWrapper[] = []

async function mountApp(transport: ShellTransport, links: IssueLinkSource, path = '/board') {
  await router.push(path)
  await router.isReady()
  const wrapper = mount(App, {
    attachTo: document.body,
    global: {
      plugins: [createPinia(), router],
      provide: {
        [kanbanTransportKey as symbol]: transport,
        [issueLinkSourceKey as symbol]: links,
      },
    },
  })
  mounted.push(wrapper)
  await flushPromises()
  return wrapper
}

function running(options: HarnessOptions = {}) {
  return harness({ tickets: tickets(), ...options })
}

function drawerHeading(): string | null | undefined {
  return document.querySelector('[role="dialog"] h2')?.textContent
}

function issueReads(query: ReturnType<typeof harness>['query'], identifier: string): number {
  return query.mock.calls.filter(
    ([name, request]) =>
      name === 'issue.get' && (request as { identifier: string }).identifier === identifier,
  ).length
}

const NOT_FOUND = { code: 'not_found', message: 'CORE-999 was not found' }

function unknownKey(name: string, request: unknown): Promise<unknown> | undefined {
  return name === 'issue.get' && (request as { identifier: string }).identifier === 'CORE-999'
    ? Promise.reject(NOT_FOUND)
    : undefined
}

// The core's Project register, answered only once the test lets it:
// every other read answers at once.
function heldRegister(answer: 'projects' | 'refusal' = 'projects') {
  const waiting: Array<() => void> = []
  let held = true
  return {
    read(): Promise<unknown> {
      return new Promise((resolve, reject) => {
        const settle = () =>
          answer === 'projects'
            ? resolve({ projects: [coreProject, edgeProject] })
            : reject({ code: 'unavailable', message: 'the Project register could not be read' })
        if (held) waiting.push(settle)
        else settle()
      })
    },
    release() {
      held = false
      for (const settle of waiting.splice(0)) settle()
    },
  }
}

const COMMENT: CommentListItem = {
  id: 1,
  author: 'lin',
  body: 'Archive the March exports first.',
  created_at: '2026-01-01T00:00:00.000000Z',
  version: 1,
}

beforeEach(() => {
  localStorage.clear()
})

afterEach(() => {
  for (const wrapper of mounted.splice(0)) wrapper.unmount()
  document.body.innerHTML = ''
})

describe('issue links', () => {
  it('opens the issue a link launched the app with once the core answers', async () => {
    // The WebView boots before the shell's link to the core is up, so
    // every first read fails with the link's own refusal.
    let linkUp = false
    const shell = running({
      override: () =>
        linkUp
          ? undefined
          : Promise.reject({
              code: 'internal',
              message: 'the core connection is not up; retry once it connects',
            }),
    })
    await mountApp(shell.transport, shellLinks('EDGE-3').source)

    expect(router.currentRoute.value.fullPath).toBe('/board')
    expect(issueReads(shell.query, 'EDGE-3'), 'the link waits for the core').toBe(0)

    linkUp = true
    shell.connection('connected')
    await flushPromises()

    expect(router.currentRoute.value.path).toBe('/projects/2/board')
    expect(router.currentRoute.value.query.ticket).toBe('11')
    expect(drawerHeading()).toContain('EDGE-3')
    expect(drawerHeading()).toContain('Edge review')
  })

  it('opens a launch link at once when the core is already up', async () => {
    const shell = running()
    await mountApp(shell.transport, shellLinks('CORE-12').source)

    expect(router.currentRoute.value.fullPath).toBe('/projects/1/board?ticket=7')
    expect(drawerHeading()).toContain('CORE-12')
  })

  it("shows a cold link's issue once the Project register answers", async () => {
    const register = heldRegister()
    const shell = running({
      override: (name) => {
        if (name === 'project.list') return register.read()
        if (name === 'comment.list') return Promise.resolve({ comments: [COMMENT], next_cursor: null })
        return undefined
      },
    })
    await mountApp(shell.transport, shellLinks('CORE-12').source)
    const beforeTheRegister = router.currentRoute.value.fullPath

    register.release()
    await flushPromises()

    expect(router.currentRoute.value.fullPath).toBe('/projects/1/board?ticket=7')
    expect(drawerHeading()).toContain('CORE-12')
    const panel = document.querySelector('[data-testid="issue-detail-panel"]')
    expect(panel, "the drawer shows the issue's panel").not.toBeNull()
    expect(
      panel?.querySelector('[data-testid="issue-description-body"]')?.textContent?.trim(),
    ).toBe('Archive the old exports')
    expect(panel?.querySelector('[data-testid="issue-detail-priority"]')?.textContent?.trim()).toBe(
      '3',
    )
    expect(panel?.querySelector('[data-testid="issue-comment-1"]')?.textContent).toContain(
      'Archive the March exports first.',
    )
    expect(beforeTheRegister, 'the link waits for the register').toBe('/board')
  })

  it('opens only the newest link once the Project register answers', async () => {
    const register = heldRegister()
    const shell = running({
      override: (name) => (name === 'project.list' ? register.read() : undefined),
    })
    const links = shellLinks()
    const wrapper = await mountApp(shell.transport, links.source)

    links.announce('EDGE-3')
    await flushPromises()
    links.announce('CORE-12')
    await flushPromises()
    register.release()
    await flushPromises()

    expect(router.currentRoute.value.fullPath).toBe('/projects/1/board?ticket=7')
    expect(drawerHeading()).toContain('CORE-12')
    expect(issueReads(shell.query, 'EDGE-3'), 'the older link is never resolved').toBe(0)
    expect(wrapper.find('[data-testid="board-error"]').exists()).toBe(false)
  })

  it("reports a Project register that cannot be read as the board's error", async () => {
    const register = heldRegister('refusal')
    const shell = running({
      override: (name) => (name === 'project.list' ? register.read() : undefined),
    })
    const wrapper = await mountApp(shell.transport, shellLinks('CORE-12').source)

    register.release()
    await flushPromises()

    expect(router.currentRoute.value.fullPath).toBe('/board')
    expect(wrapper.get('[data-testid="board-error"]').text()).toContain(
      'the Project register could not be read',
    )
    expect(document.querySelector('[role="dialog"]')).toBeNull()
    expect(issueReads(shell.query, 'CORE-12'), 'no issue opens without its Project').toBe(0)
  })

  it('moves the running app to the Project and drawer each link names', async () => {
    const shell = running()
    const links = shellLinks()
    const wrapper = await mountApp(shell.transport, links.source)
    expect(drawerHeading()).toBeUndefined()

    links.announce('EDGE-3')
    await flushPromises()

    expect(router.currentRoute.value.fullPath).toBe('/projects/2/board?ticket=11')
    expect(wrapper.get('[data-testid="board-title"]').text()).toBe('Edge tooling')
    expect(drawerHeading()).toContain('EDGE-3')

    links.announce('CORE-12')
    await flushPromises()

    expect(router.currentRoute.value.fullPath).toBe('/projects/1/board?ticket=7')
    expect(wrapper.get('[data-testid="board-title"]').text()).toBe('Control plane')
    expect(drawerHeading()).toContain('CORE-12')
  })

  for (const late of ['answer', 'refusal'] as const) {
    it(`opens the newest link when an older link's ${late} comes late`, async () => {
      // The first read of EDGE-3 — the older link's — is held back.
      let settleEdge: (() => void) | undefined
      const shell = running({
        override: (name, request) => {
          if (name !== 'issue.get' || settleEdge !== undefined) return undefined
          if ((request as { identifier: string }).identifier !== 'EDGE-3') return undefined
          return new Promise((resolve, reject) => {
            settleEdge =
              late === 'answer'
                ? () => resolve({ identifier: 'EDGE-3', project_id: 2, ticket_id: 11 } as IssueRecord)
                : () => reject({ code: 'unavailable', message: 'the core is offline' })
          })
        },
      })
      const links = shellLinks()
      const wrapper = await mountApp(shell.transport, links.source)

      links.announce('EDGE-3')
      await flushPromises()
      links.announce('CORE-12')
      await flushPromises()
      settleEdge?.()
      await flushPromises()

      expect(router.currentRoute.value.fullPath).toBe('/projects/1/board?ticket=7')
      expect(drawerHeading()).toContain('CORE-12')
      expect(wrapper.find('[data-testid="board-error"]').exists()).toBe(false)
    })
  }

  it("reports a key the core cannot find as the board's error", async () => {
    const shell = running({ override: unknownKey })
    const links = shellLinks()
    const wrapper = await mountApp(shell.transport, links.source, '/projects/1/board')

    links.announce('CORE-999')
    await flushPromises()

    expect(router.currentRoute.value.fullPath).toBe('/projects/1/board')
    expect(wrapper.get('[data-testid="board-error"]').text()).toContain('CORE-999 was not found')
    expect(document.querySelector('[role="dialog"]')).toBeNull()
    expect(wrapper.find('[data-testid="kanban-card-7"]').exists()).toBe(true)
  })

  it('brings an unknown key to the board from another surface', async () => {
    const shell = running({ override: unknownKey })
    const links = shellLinks()
    const wrapper = await mountApp(shell.transport, links.source, '/attention')

    links.announce('CORE-999')
    await flushPromises()

    expect(router.currentRoute.value.fullPath).toBe('/board')
    expect(wrapper.get('[data-testid="board-error"]').text()).toContain('CORE-999 was not found')
  })

  it('lets the error go once the operator moves on or a link opens', async () => {
    const shell = running({ override: unknownKey })
    const links = shellLinks()
    const wrapper = await mountApp(shell.transport, links.source)

    links.announce('CORE-999')
    await flushPromises()
    expect(wrapper.find('[data-testid="board-error"]').exists()).toBe(true)

    await router.push('/projects/1/board')
    await flushPromises()
    expect(wrapper.find('[data-testid="board-error"]').exists()).toBe(false)

    await router.push('/board')
    await flushPromises()
    expect(wrapper.find('[data-testid="board-error"]').exists(), 'a report never comes back').toBe(
      false,
    )

    links.announce('CORE-999')
    await flushPromises()
    expect(wrapper.find('[data-testid="board-error"]').exists()).toBe(true)

    links.announce('CORE-12')
    await flushPromises()
    expect(wrapper.find('[data-testid="board-error"]').exists()).toBe(false)
    expect(drawerHeading()).toContain('CORE-12')
  })
})
