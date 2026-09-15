import { createRouter, createWebHistory } from 'vue-router'
import BoardView from '../views/BoardView.vue'
import HomeView from '../views/HomeView.vue'
import InitiativesView from '../views/InitiativesView.vue'
import PlanningView from '../views/PlanningView.vue'
import RegisterView from '../views/RegisterView.vue'
import HerdrSettingsView from '../views/HerdrSettingsView.vue'
import CapacitySettingsView from '../views/CapacitySettingsView.vue'
import ProfilesView from '../views/ProfilesView.vue'
import ProjectSettingsView from '../views/ProjectSettingsView.vue'
import WorkspacesIndexView from '../views/WorkspacesIndexView.vue'
import WorkspacesView from '../views/WorkspacesView.vue'
import HealthDashboardView from '../views/HealthDashboardView.vue'
import SpecEditorView from '../views/SpecEditorView.vue'
import DependencyEditorView from '../views/DependencyEditorView.vue'
import AttentionInboxView from '../views/AttentionInboxView.vue'

// The route catalogue the shell's rail and the command palette read.
// The board is the front door; every operational surface the
// application already had keeps its route. The Ticket editor is not
// one of them: it is a dialog off New Ticket, off a Spec's uncovered
// Story, and off the drawer's Edit, and has no destination of its own
// (KAN-T139-AC1).
const router = createRouter({
  history: createWebHistory(),
  routes: [
    { path: '/', redirect: '/board' },
    { path: '/board', name: 'global-board', component: BoardView },
    { path: '/projects/:projectId/board', name: 'board', component: BoardView },
    { path: '/activity', name: 'activity', component: HomeView },
    { path: '/attention', name: 'attention-inbox', component: AttentionInboxView },
    { path: '/planning', name: 'planning', component: PlanningView },
    { path: '/planning/specs', name: 'planning-specs', component: SpecEditorView },
    {
      path: '/planning/dependencies',
      name: 'planning-dependencies',
      component: DependencyEditorView,
    },
    { path: '/workspaces', name: 'workspaces-index', component: WorkspacesIndexView },
    { path: '/projects/:projectId/workspaces', name: 'workspaces', component: WorkspacesView },
    { path: '/settings/profiles', name: 'profiles', component: ProfilesView },
    { path: '/settings/herdr', name: 'herdr-settings', component: HerdrSettingsView },
    { path: '/settings/capacity', name: 'capacity-settings', component: CapacitySettingsView },
    { path: '/register', name: 'register', component: RegisterView },
    {
      path: '/projects/:projectId/settings',
      name: 'project-settings',
      component: ProjectSettingsView,
    },
    { path: '/initiatives', name: 'initiatives', component: InitiativesView },
    { path: '/health', name: 'health', component: HealthDashboardView },
  ],
})

export default router
