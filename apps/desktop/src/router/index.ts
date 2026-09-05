import { createRouter, createWebHistory } from 'vue-router'
import HomeView from '../views/HomeView.vue'
import InitiativesView from '../views/InitiativesView.vue'
import PlanningView from '../views/PlanningView.vue'
import RegisterView from '../views/RegisterView.vue'
import HerdrSettingsView from '../views/HerdrSettingsView.vue'
import WorkspacesView from '../views/WorkspacesView.vue'

const router = createRouter({
  history: createWebHistory(),
  routes: [
    { path: '/', name: 'home', component: HomeView },
    { path: '/initiatives', name: 'initiatives', component: InitiativesView },
    { path: '/register', name: 'register', component: RegisterView },
    { path: '/settings/herdr', name: 'herdr-settings', component: HerdrSettingsView },
    { path: '/planning', name: 'planning', component: PlanningView },
    { path: '/projects/:projectId/workspaces', name: 'workspaces', component: WorkspacesView },
  ],
})

export default router
