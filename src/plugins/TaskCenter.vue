<script setup lang="ts">
import { onMounted, onUnmounted, ref } from 'vue'
import { listen } from '@tauri-apps/api/event'
import type { UnlistenFn } from '@tauri-apps/api/event'
import { restartDaemon } from '../daemon'
import type { PluginTaskView } from './plugins'

const emit = defineEmits<{ notice: [text: string] }>()

// 注意：任务状态只在内存里（窗口常驻不销毁；webview 被手动刷新时任务视图会从零开始，
// daemon 侧任务仍会继续跑完——可接受的已知边界，spec §9 同级）。
const tasks = ref<PluginTaskView[]>([])
const restarting = ref(false)
let unlisten: UnlistenFn | undefined

onMounted(async () => {
  unlisten = await listen<PluginTaskView>('plugins://task', (event) => {
    const view = event.payload
    const index = tasks.value.findIndex((t) => t.taskId === view.taskId)
    if (index >= 0) tasks.value[index] = view
    else tasks.value.push(view)
  })
})

onUnmounted(() => {
  unlisten?.()
})

function tail(task: PluginTaskView): string {
  return task.outputTail.slice(-8).join('\n')
}

function statusLabel(task: PluginTaskView): string {
  switch (task.status) {
    case 'running':
      return '执行中'
    case 'done':
      return '完成'
    case 'failed':
      return `失败（退出码 ${task.exitCode ?? '？'}）`
  }
}

async function restart(): Promise<void> {
  restarting.value = true
  try {
    await restartDaemon()
    emit('notice', 'daemon 正在重启，就绪后插件变更生效')
  } catch (err) {
    emit('notice', `重启失败：${String(err)}`)
  } finally {
    restarting.value = false
  }
}
</script>

<template>
  <div class="tasks">
    <p v-if="!tasks.length" class="waiting">还没有安装/移除任务。</p>
    <article v-for="task in tasks" :key="task.taskId" class="task-card" :data-status="task.status">
      <header>
        <span v-if="task.status === 'running'" class="spinner"></span>
        <strong>{{ task.kind === 'install' ? '安装' : '移除' }} {{ task.spec }}</strong>
        <span class="phase">{{ statusLabel(task) }}</span>
      </header>
      <pre v-if="task.outputTail.length" class="task-out">{{ tail(task) }}</pre>
      <footer v-if="task.status === 'done'" class="done-banner">
        <span>需重启 daemon 后生效。</span>
        <button class="mini primary" :disabled="restarting" @click="restart">
          {{ restarting ? '重启中…' : '重启 daemon' }}
        </button>
      </footer>
    </article>
  </div>
</template>
