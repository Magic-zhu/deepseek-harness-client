<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { describeSettings } from './plugins'
import type { SettingsNamespaceView } from './plugins'

const namespaces = ref<SettingsNamespaceView[]>([])
const writable = ref(false)
const loadError = ref<string | null>(null)
const loading = ref(true)

async function reload(): Promise<void> {
  loading.value = true
  try {
    const desc = await describeSettings()
    namespaces.value = desc.namespaces
    writable.value = desc.writable
    loadError.value = null
  } catch (err) {
    loadError.value = String(err)
  } finally {
    loading.value = false
  }
}

onMounted(() => void reload())

function pretty(value: unknown): string {
  return JSON.stringify(value ?? {}, null, 2)
}
</script>

<template>
  <div class="settings">
    <p v-if="loading" class="waiting">正在读取设置命名空间…</p>
    <p v-else-if="loadError" class="error">{{ loadError }}</p>
    <template v-else>
      <p v-if="!writable" class="hint">当前设置为只读（上游声明 writable=false）。</p>
      <p v-if="!namespaces.length" class="waiting">没有设置命名空间。</p>
      <article v-for="ns in namespaces" :key="ns.ns" class="ns-card">
        <header>
          <code class="mono">{{ ns.ns }}</code>
          <span class="badge" :data-applies="ns.applies">
            {{ ns.applies === 'live' ? '保存即生效' : '重启后生效' }}
          </span>
        </header>
        <details>
          <summary>当前值（已脱敏）</summary>
          <pre class="json">{{ pretty(ns.value) }}</pre>
        </details>
        <details>
          <summary>schema</summary>
          <pre class="json">{{ pretty(ns.schema) }}</pre>
        </details>
        <div v-if="ns.secrets.length" class="secrets">
          <h3>密钥槽位</h3>
          <div v-for="slot in ns.secrets" :key="slot.path.join('.')" class="secret-row">
            <code class="mono">{{ slot.path.join('.') }}</code>
            <span :class="['badge', slot.set ? 'on' : 'off']">{{ slot.set ? '已配置' : '未配置' }}</span>
          </div>
        </div>
      </article>
    </template>
  </div>
</template>
