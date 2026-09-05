import { createRouter, createWebHistory } from 'vue-router'
import HomeView from '../views/HomeView.vue'
import InitiativesView from '../views/InitiativesView.vue'
import RegisterView from '../views/RegisterView.vue'
import HerdrSettingsView from '../views/HerdrSettingsView.vue'

const router = createRouter({
  history: createWebHistory(),
  routes: [
    { path: '/', name: 'home', component: HomeView },
    { path: '/initiatives', name: 'initiatives', component: InitiativesView },
    { path: '/register', name: 'register', component: RegisterView },
    { path: '/settings/herdr', name: 'herdr-settings', component: HerdrSettingsView },
  ],
})

export default router
