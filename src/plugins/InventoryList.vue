<script setup lang="ts">
import type { PluginEntry } from './plugins'

defineProps<{ entries: PluginEntry[] }>()

function phaseLabel(entry: PluginEntry): string {
  switch (entry.fiberPhase) {
    case 'pending':
      return '等待加载'
    case 'loading':
      return '加载中'
    case 'active':
      return '运行中'
    case 'failed':
      return '加载失败'
    case 'unloading':
      return '卸载中'
    case null:
      return '未加载'
  }
}
</script>

<template>
  <div class="inventory">
    <p v-if="!entries.length" class="waiting">清单为空。</p>
    <table v-else class="table">
      <thead>
        <tr>
          <th>插件</th>
          <th>模块</th>
          <th>状态</th>
          <th>启用</th>
        </tr>
      </thead>
      <tbody>
        <tr v-for="entry in entries" :key="entry.entryId">
          <td class="mono">{{ entry.entryId }}</td>
          <td class="mono dim">{{ entry.moduleName }}</td>
          <td><span class="phase" :data-phase="entry.fiberPhase ?? 'none'">{{ phaseLabel(entry) }}</span></td>
          <td><span :class="['badge', entry.enabled ? 'on' : 'off']">{{ entry.enabled ? '启用' : '禁用' }}</span></td>
        </tr>
      </tbody>
    </table>
  </div>
</template>
