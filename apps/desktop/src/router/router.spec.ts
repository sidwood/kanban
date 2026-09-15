import { describe, expect, it } from 'vitest'
import router from './index'

// Every operational surface the application already had stays
// reachable through the shell's route catalogue; the prototype's
// scaffolding routes never exist.
const SURFACES = [
  '/board',
  '/projects/1/board',
  '/activity',
  '/attention',
  '/planning',
  '/planning/specs',
  '/planning/dependencies',
  '/workspaces',
  '/projects/1/workspaces',
  '/settings/profiles',
  '/settings/herdr',
  '/settings/capacity',
  '/register',
  '/initiatives',
  '/health',
]

describe('the route catalogue', () => {
  it('resolves every operational surface', () => {
    for (const path of SURFACES) {
      const resolved = router.resolve(path)
      expect(resolved.matched.length, path).toBeGreaterThan(0)
      expect(resolved.matched[0]?.redirect, path).toBeUndefined()
    }
  })

  it('sends the root to the board', async () => {
    await router.push('/')
    expect(router.currentRoute.value.path).toBe('/board')
  })

  it('has no route for the prototype scaffolding', () => {
    const paths = router.getRoutes().map((route) => route.path)
    for (const path of paths) {
      expect(path).not.toMatch(/states|gallery|prototype|editors/)
    }
  })

  // The Ticket editor is a dialog off context and a shortcut, so it
  // has no destination to resolve at all (KAN-T139-AC1).
  it('has no Ticket editor destination', () => {
    expect(router.resolve('/planning/tickets').matched).toHaveLength(0)
  })
})
