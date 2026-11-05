import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it } from 'vitest'
import router from '../router'
import { harness, ticket } from '../test/shell-harness'
import { useIssueLinkStore } from './issue-link'

function shell() {
  return harness({
    tickets: [
      ticket(),
      ticket({ id: 11, number: 3, project_id: 2, title: 'Edge review' }),
    ],
  })
}

function issueReads(query: ReturnType<typeof harness>['query']): unknown[] {
  return query.mock.calls.filter(([name]) => name === 'issue.get').map(([, request]) => request)
}

beforeEach(async () => {
  setActivePinia(createPinia())
  await router.push('/board')
  await router.isReady()
})

describe('the issue-link store', () => {
  it('opens only the newest key while several wait for the core', async () => {
    const { transport, query } = shell()
    const links = useIssueLinkStore()

    links.receive('CORE-12')
    links.receive('EDGE-3')
    await links.openPending(transport, router)

    expect(issueReads(query)).toStrictEqual([{ identifier: 'EDGE-3' }])
    expect(router.currentRoute.value.fullPath).toBe('/projects/2/board?ticket=11')
    expect(links.pending).toBeNull()
  })

  it('opens nothing when no key is waiting', async () => {
    const { transport, query } = shell()
    const links = useIssueLinkStore()

    await links.openPending(transport, router)

    expect(issueReads(query)).toStrictEqual([])
    expect(router.currentRoute.value.fullPath).toBe('/board')
  })

  it("keeps the core's refusal for the route it was reported on", async () => {
    const { transport } = harness({
      override: (name) =>
        name === 'issue.get'
          ? Promise.reject({ code: 'not_found', message: 'CORE-999 was not found' })
          : undefined,
    })
    const links = useIssueLinkStore()

    links.receive('CORE-999')
    await links.openPending(transport, router)
    links.follow('/board')

    expect(links.error).toBe('CORE-999 was not found')

    links.follow('/projects/1/board')

    expect(links.error).toBeNull()
  })
})
