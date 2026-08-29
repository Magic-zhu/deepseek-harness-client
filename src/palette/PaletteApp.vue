<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { listen } from '@tauri-apps/api/event'
import type { UnlistenFn } from '@tauri-apps/api/event'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { PALETTE_COMMANDS, TRAY_CHANNELS, filterCommands, runPaletteCommand } from '../palette'
import type { CommandId, PaletteCommand } from '../palette'

const query = ref<string>('')
const selectedIndex = ref<number>(0)
const inputRef = ref<HTMLInputElement | null>(null)
let unlisteners: UnlistenFn[] = []

const filtered = computed<PaletteCommand[]>(() => filterCommands(PALETTE_COMMANDS, query.value))

function clampSelection(): void {
  if (filtered.value.length === 0) {
    selectedIndex.value = 0
    return
  }
  if (selectedIndex.value >= filtered.value.length) {
    selectedIndex.value = filtered.value.length - 1
  }
  if (selectedIndex.value < 0) {
    selectedIndex.value = 0
  }
}

async function focusInput(): Promise<void> {
  await nextTick()
  inputRef.value?.focus()
  inputRef.value?.select()
}

function reset(): void {
  query.value = ''
  selectedIndex.value = 0
}

async function pick(id: CommandId): Promise<void> {
  // 先关命令面板再跑命令：避免 `open_plugins_window` 在 Rust 端
  // `set_focus()` 抢回焦点、覆盖掉 palette 窗口的 hide 行为——用户
  // 体感是"选了但面板没关"。hide 是 IPC，await 之会让 hide 真的
  // 发到宿主再走下一步；之后命令成功 / 失败都不影响 palette 已消失。
  await close()
  try {
    await runPaletteCommand(id)
  } catch (err) {
    // eslint-disable-next-line no-console
    console.error('palette command failed', err)
  }
}

async function close(): Promise<void> {
  try {
    await getCurrentWindow().hide()
  } catch {
    // 失焦关闭路径上 hide 可能失败，忽略。
  }
}

function onKey(event: KeyboardEvent): void {
  if (event.key === 'Escape') {
    event.preventDefault()
    void close()
    return
  }
  if (filtered.value.length === 0) return
  if (event.key === 'ArrowDown') {
    event.preventDefault()
    selectedIndex.value = (selectedIndex.value + 1) % filtered.value.length
    return
  }
  if (event.key === 'ArrowUp') {
    event.preventDefault()
    selectedIndex.value =
      (selectedIndex.value - 1 + filtered.value.length) % filtered.value.length
    return
  }
  if (event.key === 'Enter') {
    event.preventDefault()
    const target = filtered.value[selectedIndex.value]
    if (target) void pick(target.id)
  }
}

function selectByMouse(index: number): void {
  selectedIndex.value = index
}

function onListDblClick(index: number): void {
  const target = filtered.value[index]
  if (target) void pick(target.id)
}

onMounted(async () => {
  // 监听 Rust 端 `palette://open` 事件：托盘单击 / 主窗口快捷键触发。
  const subs = await Promise.all(
    TRAY_CHANNELS.map((name) =>
      listen<void>(name, () => {
        reset()
        void focusInput()
      }),
    ),
  )
  unlisteners.push(...subs)
  await focusInput()
})

onUnmounted(() => {
  for (const unlisten of unlisteners) unlisten()
})

// 同步 query 变化对 selectedIndex 的影响
watch(filtered, clampSelection)
</script>

<template>
  <main class="palette">
    <div class="palette-card">
      <input
        ref="inputRef"
        v-model="query"
        class="input"
        type="text"
        spellcheck="false"
        autocomplete="off"
        placeholder="输入命令…"
        @keydown="onKey"
      />
      <ul v-if="filtered.length" class="list" role="listbox">
        <li
          v-for="(cmd, index) in filtered"
          :key="cmd.id"
          :class="['item', { active: index === selectedIndex }]"
          role="option"
          :aria-selected="index === selectedIndex"
          @mouseenter="selectByMouse(index)"
          @click="selectByMouse(index)"
          @dblclick="onListDblClick(index)"
        >
          <span class="title">{{ cmd.title }}</span>
          <span class="meta">
            <span class="hint">{{ cmd.hint }}</span>
            <span v-if="cmd.shortcut !== '—'" class="shortcut">{{ cmd.shortcut }}</span>
          </span>
        </li>
      </ul>
      <div v-else class="empty">没有匹配的命令</div>
    </div>
  </main>
</template>
