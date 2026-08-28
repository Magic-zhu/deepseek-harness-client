<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { listen } from '@tauri-apps/api/event'
import type { UnlistenFn } from '@tauri-apps/api/event'
import { DAEMON_CHANNELS, fetchStatus } from '../daemon'
import type { DaemonEvent, DaemonStatus } from '../daemon'
import InventoryList from './InventoryList.vue'
import DynamicList from './DynamicList.vue'
import InstallDialog from './InstallDialog.vue'
import TaskCenter from './TaskCenter.vue'
import { fetchDynamicInventory, fetchInventory, setPluginEnabled, usePolling } from './plugins'
import type { DynamicPluginRow, PendingIntent, PluginEntry } from './plugins'

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

const entries = ref<PluginEntry[]>([])
const dynamicRows = ref<DynamicPluginRow[]>([])
const loadError = ref<string | null>(null)

const pending = ref<Record<string, PendingIntent>>({})
const notice = ref<string | null>(null)
let noticeTimer: number | undefined

function showNotice(text: string): void {
  notice.value = text
  window.clearTimeout(noticeTimer)
  noticeTimer = window.setTimeout(() => {
    notice.value = null
  }, 6000)
}

const dialog = ref<'install' | 'remove' | null>(null)

function onTaskSubmitted(): void {
  tab.value = 'tasks'
}

async function onToggle(entry: PluginEntry): Promise<void> {
  const intent = !entry.enabled
  try {
    await setPluginEnabled(entry.entryId, intent)
    pending.value = { ...pending.value, [entry.entryId]: { intent, deadline: Date.now() + 10_000 } }
  } catch (err) {
    showNotice(`写入开关失败：${String(err)}`)
  }
}

/** 每轮轮询后核对未决意图：生效即清除；10s 未生效提示 HMR 可能未应用。 */
function settlePending(): void {
  if (!Object.keys(pending.value).length) return
  const now = Date.now()
  const next = { ...pending.value }
  for (const [entryId, p] of Object.entries(next)) {
    const current = entries.value.find((e) => e.entryId === entryId)
    if (current && current.enabled === p.intent) {
      delete next[entryId]
    } else if (now > p.deadline) {
      delete next[entryId]
      showNotice(`插件 ${entryId} 的开关未在 10 秒内生效（HMR 可能未应用），可尝试重启 daemon`)
    }
  }
  pending.value = next
}

async function loadInventories(): Promise<void> {
  try {
    const snap = await fetchInventory()
    entries.value = snap.entries
    settlePending()
    loadError.value = null
  } catch (err) {
    loadError.value = String(err)
  }
  try {
    dynamicRows.value = await fetchDynamicInventory()
  } catch {
    /* 动态清单失败保留旧数据；主错误条已提示 */
  }
}

usePolling(loadInventories)
</script>

<template>
  <main class="plugins">
    <header class="bar">
      <h1>插件管理</h1>
      <div class="bar-actions">
        <button class="mini" @click="dialog = 'install'">安装插件</button>
        <button class="mini" @click="dialog = 'remove'">移除插件</button>
      </div>
      <span class="daemon-state" :data-ready="daemonReady">{{ statusText }}</span>
    </header>
    <p v-if="notice" class="notice">{{ notice }}</p>
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
      <template v-else>
        <p v-if="loadError" class="error">{{ loadError }}</p>
        <InventoryList v-if="tab === 'inventory'" :entries="entries" :pending="pending" @toggle="onToggle" />
        <DynamicList v-else-if="tab === 'dynamic'" :rows="dynamicRows" />
        <TaskCenter v-else-if="tab === 'tasks'" @notice="showNotice" />
        <p v-else class="waiting">（后续切片交付此面板）</p>
      </template>
    </section>
    <InstallDialog v-if="dialog" :mode="dialog" @close="dialog = null" @submitted="onTaskSubmitted" />
  </main>
</template>
