import { invoke } from '@tauri-apps/api/core'

/** `palette://*` 事件通道全集。 */
export const TRAY_CHANNELS = ['palette://open'] as const

export type CommandId = 'show_plugins' | 'show_main' | 'restart' | 'quit'

export interface PaletteCommand {
  id: CommandId
  title: string
  hint: string
  shortcut: string
}

/** 命令面板里展示的全部命令。按显示顺序排列。 */
export const PALETTE_COMMANDS: PaletteCommand[] = [
  {
    id: 'show_plugins',
    title: '打开插件管理',
    hint: '插件清单 / 动态插件 / 设置 / 任务',
    shortcut: 'Ctrl+Shift+P',
  },
  {
    id: 'show_main',
    title: '打开主窗口',
    hint: '显示 dsh Web UI 主窗口',
    shortcut: '—',
  },
  {
    id: 'restart',
    title: '重启 dsh 守护进程',
    hint: '停止并重新拉起上游 dsh',
    shortcut: '—',
  },
  {
    id: 'quit',
    title: '退出',
    hint: '结束 DeepSeek Harness 进程',
    shortcut: '—',
  },
]

/** 执行一个命令面板命令。通过 Tauri IPC 走 Rust。 */
export async function runPaletteCommand(id: CommandId): Promise<void> {
  switch (id) {
    case 'show_plugins':
      await invoke('open_plugins_window')
      return
    case 'show_main':
      await invoke('show_main_window')
      return
    case 'restart':
      await invoke('daemon_restart')
      return
    case 'quit':
      await invoke('app_quit')
      return
  }
}

/** 简单的子串过滤：忽略大小写、忽略全/半角空格。 */
export function filterCommands(commands: PaletteCommand[], query: string): PaletteCommand[] {
  const needle = query.trim().toLowerCase()
  if (!needle) return commands
  return commands.filter(
    (c) => c.title.toLowerCase().includes(needle) || c.hint.toLowerCase().includes(needle),
  )
}
