import { createRouter, createWebHistory } from 'vue-router'
import HomeView from '../views/HomeView.vue'
import InitiativesView from '../views/InitiativesView.vue'

const router = createRouter({
  history: createWebHistory(),
  routes: [
    { path: '/', name: 'home', component: HomeView },
    { path: '/initiatives', name: 'initiatives', component: InitiativesView },
  ],
})

export default router
