<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import type { UnlistenFn } from '@tauri-apps/api/event'
import {
  DAEMON_CHANNELS,
  PREFLIGHT_CHANNELS,
  fetchLogTail,
  fetchPreflight,
  fetchStatus,
  restartDaemon,
} from './daemon'
import type {
  CrashedEvent,
  DaemonEvent,
  DaemonStatus,
  LogLine,
  PreflightReport,
} from './daemon'

const status = ref<DaemonStatus | null>(null)
const lastCrash = ref<CrashedEvent | null>(null)
const tail = ref<LogLine[]>([])
const restarting = ref(false)
const preflight = ref<PreflightReport | null>(null)
const reprobing = ref(false)
let unlisteners: UnlistenFn[] = []

function onEvent(payload: DaemonEvent): void {
  switch (payload.type) {
    case 'starting':
    case 'ready':
      lastCrash.value = null
      break
    case 'crashed':
      lastCrash.value = payload
      break
    case 'log':
      tail.value = [...tail.value, { stream: payload.stream, line: payload.line }].slice(-200)
      break
    case 'stopped':
      break
  }
  void refreshStatus()
}

async function refreshStatus(): Promise<void> {
  const next = await fetchStatus().catch(() => null)
  if (next) status.value = next
}

const preflightFailed = computed<boolean>(
  () => preflight.value !== null && !preflight.value.engineOk,
)

const headline = computed<string>(() => {
  if (preflightFailed.value) return 'Node 运行时不符合上游要求'
  const s = status.value
  if (!s) return '正在连接…'
  switch (s.state) {
    case 'starting':
      return s.attempt > 1 ? `正在启动 dsh 守护进程（第 ${s.attempt} 次尝试）…` : '正在启动 dsh 守护进程…'
    case 'running':
      return s.port !== null ? `守护进程已就绪（端口 ${s.port}），正在进入…` : '守护进程已就绪，正在进入…'
    case 'backoff':
      return '守护进程退出或启动失败'
    case 'stopped':
      return '守护进程已停止'
  }
})

const detail = computed<string | null>(() => {
  if (preflightFailed.value) return preflight.value?.failure ?? null
  const s = status.value
  if (!s) return null
  if (s.state === 'backoff') {
    const reason = lastCrash.value?.reason ?? s.lastError ?? '未知原因'
    const seconds = Math.ceil((lastCrash.value?.retryInMs ?? 0) / 1000)
    return `${reason}；${seconds} 秒后自动重试`
  }
  if (s.state === 'stopped') return '点击下方按钮重新启动守护进程'
  return null
})

const inFlight = computed<boolean>(() => {
  if (preflightFailed.value) return reprobing.value
  const state = status.value?.state
  return state === 'starting' || state === 'running'
})

const failed = computed<boolean>(() => {
  if (preflightFailed.value) return true
  const state = status.value?.state
  return state === 'backoff' || state === 'stopped'
})

const showRetry = computed<boolean>(() => failed.value && !preflightFailed.value)

const preflightFacts = computed<Array<[string, string]>>(() => {
  const r = preflight.value
  if (!r) return []
  const rows: Array<[string, string]> = []
  if (r.version) rows.push(['检测到的 Node 版本', r.version])
  else rows.push(['检测到的 Node 版本', '（未读取到）'])
  rows.push(['版本来源', describeSource(r.versionSource)])
  if (r.nodePath) rows.push(['PATH 上的 node 路径', r.nodePath])
  rows.push(['上游要求', r.required])
  rows.push(['计划启动的命令', r.dshCommandDisplay])
  rows.push(['dsh 可达', r.dshReachable ? '是' : '否'])
  return rows
})

function describeSource(source: PreflightReport['versionSource']): string {
  switch (source) {
    case 'overrideBin':
      return 'DSH_CLIENT_BIN 的第一个 token'
    case 'pathNode':
      return 'PATH 上的 node'
    case 'unavailable':
      return '未找到'
  }
}

const nvmSteps = `# 推荐：使用 nvm-windows 切换 Node 版本
nvm install 24.15.0
nvm use 24.15.0

# 切回应用窗口，点击下方"重新检测"按钮
# 若已通过，退出并重新启动 dsh-client`

async function retry(): Promise<void> {
  restarting.value = true
  try {
    await restartDaemon()
    await refreshStatus()
  } finally {
    restarting.value = false
  }
}

async function openPlugins(): Promise<void> {
  await invoke('open_plugins_window').catch(() => {})
}

async function reprobe(): Promise<void> {
  reprobing.value = true
  try {
    const next = await fetchPreflight().catch(() => null)
    if (next) preflight.value = next
  } finally {
    reprobing.value = false
  }
}

onMounted(async () => {
  const preflightSubs = await Promise.all(
    PREFLIGHT_CHANNELS.map((name) =>
      listen<PreflightReport>(name, (event) => {
        preflight.value = event.payload
      }),
    ),
  )
  unlisteners.push(...preflightSubs.map((unlisten) => unlisten))

  // 后端会主动 emit，但若用户在 setup 完成前就订阅不到，先主动拉一次。
  const initial = await fetchPreflight().catch(() => null)
  if (initial && !preflight.value) preflight.value = initial

  if (!preflightFailed.value) {
    const subs = await Promise.all(
      DAEMON_CHANNELS.map((name) => listen<DaemonEvent>(name, (event) => onEvent(event.payload))),
    )
    unlisteners.push(...subs.map((unlisten) => unlisten))
    status.value = await fetchStatus().catch(() => null)
    tail.value = await fetchLogTail(60).catch(() => [])
  }
})

onUnmounted(() => {
  for (const unlisten of unlisteners) unlisten()
})
</script>

<template>
  <main class="splash">
    <section class="card" :class="{ failed }">
      <div class="mark" :class="{ spin: inFlight, error: failed }" aria-hidden="true">
        <span class="ring"></span>
        <span class="core"></span>
      </div>
      <h1>DeepSeek Harness</h1>
      <p class="headline">{{ headline }}</p>
      <p v-if="detail" class="detail">{{ detail }}</p>
      <button v-if="showRetry" class="retry" :disabled="restarting" @click="retry">
        {{ restarting ? '正在重启…' : '立即重试' }}
      </button>
      <button class="plugins-entry" @click="openPlugins">插件管理</button>
      <div v-if="preflightFailed" class="preflight">
        <dl class="kv">
          <template v-for="[key, value] in preflightFacts" :key="key">
            <dt>{{ key }}</dt>
            <dd>{{ value }}</dd>
          </template>
        </dl>
        <pre class="steps">{{ nvmSteps }}</pre>
        <button class="retry" :disabled="reprobing" @click="reprobe">
          {{ reprobing ? '正在重新检测…' : '重新检测' }}
        </button>
        <p class="hint">
          若已切换 Node 版本但仍显示此页，请完全退出本应用再重新启动（仅重新检测不会启动守护进程）。
        </p>
      </div>
      <div v-if="!preflightFailed && tail.length" class="tail" aria-label="守护进程日志尾部">
        <div v-for="(entry, index) in tail" :key="index" :class="['line', entry.stream]">
          {{ entry.line }}
        </div>
      </div>
      <footer v-if="status?.command">
        <code class="cmd">{{ status.command }}</code>
        <span class="hint">可设置环境变量 DSH_CLIENT_BIN 指定自定义启动命令</span>
      </footer>
    </section>
  </main>
</template>
