import { createRouter, createWebHistory } from 'vue-router'
import BoardView from '../views/BoardView.vue'
import HomeView from '../views/HomeView.vue'
import InitiativesView from '../views/InitiativesView.vue'
import PlanningView from '../views/PlanningView.vue'
import RegisterView from '../views/RegisterView.vue'
import HerdrSettingsView from '../views/HerdrSettingsView.vue'
import CapacitySettingsView from '../views/CapacitySettingsView.vue'
import ProfilesView from '../views/ProfilesView.vue'
import WorkspacesView from '../views/WorkspacesView.vue'
import SpecEditorView from '../views/SpecEditorView.vue'
import TicketEditorView from '../views/TicketEditorView.vue'
import DependencyEditorView from '../views/DependencyEditorView.vue'

const router = createRouter({
  history: createWebHistory(),
  routes: [
    { path: '/', name: 'home', component: HomeView },
    { path: '/initiatives', name: 'initiatives', component: InitiativesView },
    { path: '/register', name: 'register', component: RegisterView },
    { path: '/settings/herdr', name: 'herdr-settings', component: HerdrSettingsView },
    { path: '/settings/capacity', name: 'capacity-settings', component: CapacitySettingsView },
    { path: '/settings/profiles', name: 'profiles', component: ProfilesView },
    { path: '/planning', name: 'planning', component: PlanningView },
    { path: '/planning/specs', name: 'planning-specs', component: SpecEditorView },
    { path: '/planning/tickets', name: 'planning-tickets', component: TicketEditorView },
    {
      path: '/planning/dependencies',
      name: 'planning-dependencies',
      component: DependencyEditorView,
    },
    { path: '/projects/:projectId/board', name: 'board', component: BoardView },
    { path: '/projects/:projectId/workspaces', name: 'workspaces', component: WorkspacesView },
  ],
})

export default router
