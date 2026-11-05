import { createApp } from 'vue'
import { createPinia } from 'pinia'
import App from './App.vue'
import router from './router'
import { issueLinkSourceKey, tauriIssueLinks } from './core/issue-links'
import { kanbanTransportKey, tauriTransport } from './core/transport'
import './main.css'

createApp(App)
  .use(createPinia())
  .use(router)
  .provide(kanbanTransportKey, tauriTransport)
  .provide(issueLinkSourceKey, tauriIssueLinks)
  .mount('#app')
