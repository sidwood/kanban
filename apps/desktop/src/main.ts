import { createApp } from 'vue'
import { createPinia } from 'pinia'
import App from './App.vue'
import router from './router'
import { kanbanTransportKey, tauriTransport } from './core/transport'
import './main.css'

createApp(App)
  .use(createPinia())
  .use(router)
  .provide(kanbanTransportKey, tauriTransport)
  .mount('#app')
