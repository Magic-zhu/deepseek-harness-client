<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { describeSettings, mutateSettings, updateSettings } from './plugins'
import type { SettingsNamespaceView } from './plugins'

const emit = defineEmits<{ notice: [text: string] }>()

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

// ---- user 层 JSON 编辑（每个 ns 一份编辑态，保存/冲突后重置）----

interface NsEdit {
  text: string
  error: string | null
  saving: boolean
}

const edits = ref<Record<string, NsEdit>>({})

function editOf(ns: SettingsNamespaceView): NsEdit {
  if (!edits.value[ns.ns]) {
    edits.value[ns.ns] = { text: pretty(ns.user ?? {}), error: null, saving: false }
  }
  return edits.value[ns.ns]
}

function isConflict(err: unknown): boolean {
  return String(err).startsWith('[settings-conflict]')
}

async function save(ns: SettingsNamespaceView): Promise<void> {
  const edit = editOf(ns)
  let patch: unknown
  try {
    patch = JSON.parse(edit.text || '{}')
  } catch {
    edit.error = 'JSON 解析失败，请检查语法'
    return
  }
  if (patch === null || typeof patch !== 'object' || Array.isArray(patch)) {
    edit.error = '补丁必须是 JSON 对象（merge 进 user 层）'
    return
  }
  edit.saving = true
  edit.error = null
  try {
    await updateSettings(ns.ns, patch, ns.revision)
    await reload()
    delete edits.value[ns.ns]
    emit(
      'notice',
      ns.applies === 'restart' ? `已保存；${ns.ns} 需重启 daemon 后生效` : `已保存，${ns.ns} 即时生效`,
    )
  } catch (err) {
    if (isConflict(err)) {
      await reload()
      delete edits.value[ns.ns]
      const fresh = namespaces.value.find((n) => n.ns === ns.ns) ?? ns
      editOf(fresh).error = '设置已被他处修改，已载入最新值，请核对后重新保存'
    } else {
      edit.error = String(err)
    }
  } finally {
    edit.saving = false
  }
}

// ---- secret 槽位（write-only，清除走 unset）----

const secretInputs = ref<Record<string, string>>({})
const secretBusy = ref<string | null>(null)
const secretErrors = ref<Record<string, string | null>>({})

function secretKey(ns: string, path: string[]): string {
  return `${ns}//${path.join('.')}`
}

async function setSecret(ns: SettingsNamespaceView, path: string[]): Promise<void> {
  const key = secretKey(ns.ns, path)
  const value = secretInputs.value[key] ?? ''
  if (!value) return
  secretBusy.value = key
  secretErrors.value[key] = null
  try {
    await mutateSettings(ns.ns, [{ op: 'set', path, value }], ns.revision)
    secretInputs.value[key] = ''
    await reload()
    emit('notice', `密钥 ${path.join('.')} 已写入（write-only，不回显）`)
  } catch (err) {
    if (isConflict(err)) {
      await reload()
      secretErrors.value[key] = '设置已被他处修改，已刷新，请重试'
    } else {
      secretErrors.value[key] = String(err)
    }
  } finally {
    secretBusy.value = null
  }
}

async function clearSecret(ns: SettingsNamespaceView, path: string[]): Promise<void> {
  const key = secretKey(ns.ns, path)
  secretBusy.value = key
  secretErrors.value[key] = null
  try {
    await mutateSettings(ns.ns, [{ op: 'unset', path }], ns.revision)
    await reload()
    emit('notice', `密钥 ${path.join('.')} 已清除`)
  } catch (err) {
    if (isConflict(err)) {
      await reload()
      secretErrors.value[key] = '设置已被他处修改，已刷新，请重试'
    } else {
      secretErrors.value[key] = String(err)
    }
  } finally {
    secretBusy.value = null
  }
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

        <div class="editor">
          <label :for="`edit-${ns.ns}`">用户层补丁（JSON 对象，merge 进 user 层）</label>
          <textarea
            :id="`edit-${ns.ns}`"
            v-model="editOf(ns).text"
            rows="8"
            spellcheck="false"
            :disabled="!writable || editOf(ns).saving"
          ></textarea>
          <p v-if="editOf(ns).error" class="error">{{ editOf(ns).error }}</p>
          <button class="mini" :disabled="!writable || editOf(ns).saving" @click="save(ns)">
            {{ editOf(ns).saving ? '保存中…' : '保存' }}
          </button>
        </div>

        <div v-if="ns.secrets.length" class="secrets">
          <h3>密钥槽位</h3>
          <div v-for="slot in ns.secrets" :key="slot.path.join('.')">
            <div class="secret-row">
              <code class="mono">{{ slot.path.join('.') }}</code>
              <span :class="['badge', slot.set ? 'on' : 'off']">{{ slot.set ? '已配置' : '未配置' }}</span>
              <input
                v-model="secretInputs[secretKey(ns.ns, slot.path)]"
                type="password"
                placeholder="输入新值（write-only）"
                :disabled="!writable || secretBusy !== null"
              />
              <button
                class="mini"
                :disabled="!writable || secretBusy !== null || !secretInputs[secretKey(ns.ns, slot.path)]"
                @click="setSecret(ns, slot.path)"
              >
                写入
              </button>
              <button
                v-if="slot.set"
                class="mini danger"
                :disabled="!writable || secretBusy !== null"
                @click="clearSecret(ns, slot.path)"
              >
                清除
              </button>
            </div>
            <p v-if="secretErrors[secretKey(ns.ns, slot.path)]" class="error">
              {{ secretErrors[secretKey(ns.ns, slot.path)] }}
            </p>
          </div>
        </div>
      </article>
    </template>
  </div>
</template>
