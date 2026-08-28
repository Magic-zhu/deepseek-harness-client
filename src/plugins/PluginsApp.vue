<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { listen } from '@tauri-apps/api/event'
import type { UnlistenFn } from '@tauri-apps/api/event'
import { DAEMON_CHANNELS, fetchStatus } from '../daemon'
import type { DaemonEvent, DaemonStatus } from '../daemon'

/** 顶部 daemon 状态条：本窗口的一切数据都依赖 daemon 就绪。 */
const status = ref<DaemonStatus | null>(null)
let unlisteners: UnlistenFn[] = []

const daemonReady = computed<boolean>(
  () => status.value?.state === 'running' && status.value.port !== null,
)

const statusText = computed<string>(() => {
  const s = status.value
  if (!s) return '正在获取 daemon 状态…'
  switch (s.state) {
    case 'starting':
      return 'daemon 启动中…'
    case 'running':
      return daemonReady.value ? `daemon 运行中（端口 ${s.port}）` : 'daemon 运行中（等待端口）'
    case 'backoff':
      return 'daemon 崩溃重试中'
    case 'stopped':
      return 'daemon 已停止'
  }
})

async function refreshStatus(): Promise<void> {
  const next = await fetchStatus().catch(() => null)
  if (next) status.value = next
}

onMounted(async () => {
  const subs = await Promise.all(
    DAEMON_CHANNELS.map((name) => listen<DaemonEvent>(name, () => void refreshStatus())),
  )
  unlisteners.push(...subs)
  await refreshStatus()
})

onUnmounted(() => {
  for (const unlisten of unlisteners) unlisten()
})

type Tab = 'inventory' | 'dynamic' | 'settings' | 'tasks'
const tab = ref<Tab>('inventory')
const tabs: Array<[Tab, string]> = [
  ['inventory', '插件清单'],
  ['dynamic', '动态插件'],
  ['settings', '设置'],
  ['tasks', '任务'],
]
</script>

<template>
  <main class="plugins">
    <header class="bar">
      <h1>插件管理</h1>
      <span class="daemon-state" :data-ready="daemonReady">{{ statusText }}</span>
    </header>
    <nav class="tabs">
      <button
        v-for="[key, label] in tabs"
        :key="key"
        :class="['tab', { active: tab === key }]"
        @click="tab = key"
      >
        {{ label }}
      </button>
    </nav>
    <section class="panel">
      <p v-if="!daemonReady" class="waiting">等待 daemon 就绪后可加载数据。</p>
      <p v-else class="waiting">（后续切片交付此面板）</p>
    </section>
  </main>
</template>
