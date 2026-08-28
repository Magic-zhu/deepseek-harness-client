import { createApp } from 'vue'
import { getCurrentWindow } from '@tauri-apps/api/window'
import App from './App.vue'
import PluginsApp from './plugins/PluginsApp.vue'
import './styles.css'

const root = getCurrentWindow().label === 'plugins' ? PluginsApp : App
createApp(root).mount('#app')
