<script setup lang="ts">
import { ref } from 'vue'
import { installPlugin, removePlugin } from './plugins'

const props = defineProps<{ mode: 'install' | 'remove' }>()

const emit = defineEmits<{
  close: []
  submitted: [taskId: string]
}>()

const spec = ref('')
const error = ref<string | null>(null)
const busy = ref(false)

async function submit(): Promise<void> {
  error.value = null
  busy.value = true
  try {
    const action = props.mode === 'install' ? installPlugin : removePlugin
    const taskId = await action(spec.value.trim())
    emit('submitted', taskId)
    emit('close')
  } catch (err) {
    error.value = String(err)
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <div class="dialog-mask" @click.self="emit('close')">
    <div class="dialog">
      <h2>{{ mode === 'install' ? '安装插件' : '移除插件' }}</h2>
      <p v-if="mode === 'install'" class="warn">
        安装即执行任意第三方代码（npm 生命周期脚本与插件本体），请确认来源可信。
      </p>
      <p v-else class="hint">输入要移除的包名（与安装时一致）。移除后需重启 daemon 生效。</p>
      <input
        v-model="spec"
        :placeholder="mode === 'install' ? 'npm 包 spec，如 @scope/name@1.0.0 或 github:user/repo（版本请写确切值，暂不支持 ^ 范围）' : '包名，如 @scope/name'"
        :disabled="busy"
        @keydown.enter="submit"
      />
      <p v-if="error" class="error">{{ error }}</p>
      <footer>
        <button class="mini" :disabled="busy" @click="emit('close')">取消</button>
        <button class="mini primary" :disabled="busy || !spec.trim()" @click="submit">
          {{ busy ? '提交中…' : mode === 'install' ? '安装' : '移除' }}
        </button>
      </footer>
    </div>
  </div>
</template>
