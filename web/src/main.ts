import { createApp } from 'vue'
import { createPinia } from 'pinia'
import { createRouter, createWebHistory } from 'vue-router'
import App from './App.vue'
import AuthGate from './components/AuthGate.vue'
import RemoteView from './views/RemoteView.vue'
import SettingsView from './views/SettingsView.vue'
import './styles/app.css'

const router = createRouter({
  history: createWebHistory(),
  routes: [
    { path: '/', name: 'remote', component: RemoteView },
    { path: '/enroll', name: 'enroll', component: AuthGate },
    { path: '/settings/:section?', name: 'settings', component: SettingsView },
  ],
})

createApp(App).use(createPinia()).use(router).mount('#app')
