import { flushPromises } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const { invokeMock, listenMock } = vi.hoisted(() => ({
  invokeMock: vi.fn(),
  listenMock: vi.fn(),
}))
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }))
vi.mock('@tauri-apps/api/event', () => ({ listen: listenMock }))

import { tauriIssueLinks } from './issue-links'

type ShellHandler = (event: { payload: unknown }) => void

// The shell's event bridge, steerable from the test: a listener only
// attaches once the test lets `listen` resolve.
function shellEvents() {
  const handlers = new Map<string, ShellHandler>()
  const unlisten = vi.fn()
  let attach: () => void = () => undefined
  listenMock.mockImplementation((name: string, handler: ShellHandler) => {
    handlers.set(name, handler)
    return new Promise<() => void>((resolve) => {
      attach = () => resolve(unlisten)
    })
  })
  invokeMock.mockResolvedValue(null)
  return {
    attach: () => attach(),
    deliver: (payload: unknown) => handlers.get('shell://issue-link')?.({ payload }),
    unlisten,
  }
}

beforeEach(() => {
  invokeMock.mockReset()
  listenMock.mockReset()
})

describe('the shell issue-link bridge', () => {
  it('asks the shell for a held link only once its listener is attached', async () => {
    const shell = shellEvents()
    tauriIssueLinks.onIssueLink(() => undefined)
    await flushPromises()

    expect(listenMock).toHaveBeenCalledWith('shell://issue-link', expect.any(Function))
    expect(invokeMock).not.toHaveBeenCalled()

    shell.attach()
    await flushPromises()

    expect(invokeMock).toHaveBeenCalledTimes(1)
    expect(invokeMock).toHaveBeenCalledWith('issue_link_listen')
  })

  it('delivers the key each announcement names', async () => {
    const shell = shellEvents()
    const opened: string[] = []
    tauriIssueLinks.onIssueLink((link) => opened.push(link.identifier))
    shell.attach()
    await flushPromises()

    shell.deliver({ identifier: 'CORE-12' })
    shell.deliver({ identifier: 'EDGE-3' })

    expect(opened).toStrictEqual(['CORE-12', 'EDGE-3'])
  })

  it('ignores an announcement that names no key', async () => {
    const shell = shellEvents()
    const handler = vi.fn()
    tauriIssueLinks.onIssueLink(handler)
    shell.attach()
    await flushPromises()

    for (const payload of [null, 'CORE-12', {}, { identifier: 12 }, { identifier: '' }]) {
      shell.deliver(payload)
    }

    expect(handler).not.toHaveBeenCalled()
  })

  it('stops listening when stopped, even before the listener attached', async () => {
    const shell = shellEvents()
    const stop = tauriIssueLinks.onIssueLink(() => undefined)
    stop()
    shell.attach()
    await flushPromises()

    expect(shell.unlisten).toHaveBeenCalledTimes(1)
    expect(invokeMock, 'a stopped bridge leaves a held link with the shell').not.toHaveBeenCalled()
  })

  it('survives a shell that cannot be reached', async () => {
    listenMock.mockRejectedValue(new Error('no shell'))
    const stop = tauriIssueLinks.onIssueLink(() => undefined)
    await flushPromises()

    expect(invokeMock).not.toHaveBeenCalled()
    expect(() => stop()).not.toThrow()
  })
})
