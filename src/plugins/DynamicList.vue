<script setup lang="ts">
import { ref } from 'vue'
import { stopDynamicPlugin, undefineDynamicPlugin } from './plugins'
import type { DynamicPluginRow } from './plugins'

defineProps<{ rows: DynamicPluginRow[] }>()

const emit = defineEmits<{
  refresh: []
  notice: [text: string]
}>()

/** 一次只允许一个操作；删除用两段式按钮代替原生 confirm（webview 里不可靠）。 */
const acting = ref<string | null>(null)
const confirming = ref<string | null>(null)

/** 当前生效包（运行中的包优先，其次 last-success，再其次最新版本）。 */
function currentPackage(row: DynamicPluginRow) {
  const id = row.activeRun?.packageId ?? row.currentPackageId
  return row.packages.find((p) => p.packageId === id) ?? row.packages[row.packages.length - 1]
}

function halfLabel(status: string | undefined): string {
  switch (status) {
    case 'running':
      return '运行中'
    case 'waiting':
      return '等待依赖'
    case 'pending':
      return '加载中'
    case 'stopped':
      return '已停止'
    case 'failed':
      return '失败'
    case 'absent':
      return '无此半'
    default:
      return '—'
  }
}

function runStatusLabel(row: DynamicPluginRow): string {
  if (row.activeRun) return '运行中'
  switch (row.latestRun?.status) {
    case 'awaiting-approval':
      return '等待批准'
    case 'starting-host':
      return '启动中'
    case 'client-pending':
      return '等待页面'
    case 'waiting':
      return '等待依赖'
    case 'rejected':
      return '已拒绝'
    case 'failed':
      return '失败'
    case 'cancelled':
      return '已取消'
    case 'stopped':
      return '已停止'
    default:
      return '未运行'
  }
}

async function stop(row: DynamicPluginRow): Promise<void> {
  acting.value = row.pluginId
  try {
    const receipt = await stopDynamicPlugin(row.agentId, row.pluginId)
    emit('notice', receipt.ok ? '已停止' : `停止失败：${receipt.message ?? receipt.reason}`)
    emit('refresh')
  } catch (err) {
    emit('notice', `停止失败：${String(err)}`)
  } finally {
    acting.value = null
  }
}

async function undefine(row: DynamicPluginRow): Promise<void> {
  acting.value = row.pluginId
  confirming.value = null
  try {
    const receipt = await undefineDynamicPlugin(row.agentId, row.pluginId)
    emit(
      'notice',
      receipt.ok
        ? receipt.wasRunning
          ? '已删除（其运行实例已一并停止）'
          : '已删除'
        : `删除失败：${receipt.message ?? receipt.reason}`,
    )
    emit('refresh')
  } catch (err) {
    emit('notice', `删除失败：${String(err)}`)
  } finally {
    acting.value = null
  }
}
</script>

<template>
  <div class="dynamic">
    <p class="hint">动态插件由模型在会话中定义，会话级、进程内，daemon 重启即失。</p>
    <p v-if="!rows.length" class="waiting">当前没有动态插件。</p>
    <article v-for="row in rows" :key="row.pluginId" class="dyn-card">
      <header>
        <strong>{{ currentPackage(row)?.name || row.pluginId }}</strong>
        <span class="phase" :data-phase="row.activeRun ? 'active' : 'none'">{{ runStatusLabel(row) }}</span>
      </header>
      <p v-if="currentPackage(row)?.purpose" class="dim">{{ currentPackage(row)?.purpose }}</p>
      <dl class="kv">
        <dt>pluginId</dt>
        <dd class="mono">{{ row.pluginId }}</dd>
        <dt>所属会话</dt>
        <dd class="mono">{{ row.agentId }}</dd>
        <dt>包版本数</dt>
        <dd>{{ row.packages.length }}</dd>
        <dt>host 半</dt>
        <dd>
          {{ halfLabel(row.latestRun?.host.status) }}
          <template v-if="row.latestRun?.host.waitingFor?.length">
            （等待：{{ row.latestRun.host.waitingFor.join(', ') }}）
          </template>
        </dd>
        <dt>client 半</dt>
        <dd>{{ halfLabel(row.latestRun?.client.status) }}</dd>
      </dl>
      <pre v-if="row.latestRun?.error" class="error-detail">{{ row.latestRun.error.phase }}: {{ row.latestRun.error.message }}</pre>
      <footer class="actions">
        <button
          v-if="row.activeRun"
          class="mini"
          :disabled="acting !== null"
          @click="stop(row)"
        >
          {{ acting === row.pluginId ? '操作中…' : '停止' }}
        </button>
        <template v-if="confirming !== row.pluginId">
          <button class="mini danger" :disabled="acting !== null" @click="confirming = row.pluginId">删除</button>
        </template>
        <template v-else>
          <button class="mini danger" :disabled="acting !== null" @click="undefine(row)">确认删除（不可恢复）</button>
          <button class="mini" @click="confirming = null">取消</button>
        </template>
      </footer>
    </article>
  </div>
</template>
