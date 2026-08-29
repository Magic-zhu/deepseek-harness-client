import { createApp } from 'vue'
import { getCurrentWindow } from '@tauri-apps/api/window'
import App from './App.vue'
import PluginsApp from './plugins/PluginsApp.vue'
import PaletteApp from './palette/PaletteApp.vue'
import './styles.css'

const label = getCurrentWindow().label
const root = label === 'plugins' ? PluginsApp : label === 'palette' ? PaletteApp : App
createApp(root).mount('#app')
