import { invoke } from '@tauri-apps/api/core'
import { onMounted, onUnmounted } from 'vue'

/** daemon /api 透传。业务错误以 `[code] message` 字符串 reject。 */
export function apiCall<T = unknown>(method: string, payload: unknown): Promise<T> {
  return invoke<T>('dsh_api_call', { method, payload })
}

// ---- loader 插件清单（pluginInventory/list，Typert Remote；args 壳由 bridge 自动包）----

export type FiberPhase = 'pending' | 'loading' | 'active' | 'failed' | 'unloading' | null

export interface PluginEntry {
  entryId: string
  moduleName: string
  enabled: boolean
  fiberPhase: FiberPhase
}

export interface PluginInventorySnapshot {
  entries: PluginEntry[]
}

export const fetchInventory = (): Promise<PluginInventorySnapshot> =>
  apiCall('pluginInventory/list', {})

// ---- 动态插件（dynamicCordisRunner/*）----

export interface DynamicPackage {
  packageId: string
  name: string
  purpose: string
  hasHostHalf: boolean
  hasClientHalf: boolean
}

export type CordisHalfStatus = 'absent' | 'pending' | 'stopped' | 'running' | 'waiting' | 'failed'

export interface CordisHalfState {
  status: CordisHalfStatus
  waitingFor: string[]
  error?: string
}

export type CordisRunStatus =
  | 'awaiting-approval'
  | 'starting-host'
  | 'client-pending'
  | 'running'
  | 'waiting'
  | 'rejected'
  | 'failed'
  | 'cancelled'
  | 'stopped'

export interface DynamicRunAttempt {
  pluginRunId: string
  packageId: string
  mode: 'run' | 'update'
  status: CordisRunStatus
  approvalRequestId?: string
  requiresApproval?: boolean
  host: CordisHalfState
  client: CordisHalfState
  error?: { phase: string; message: string; stack?: string }
}

export interface DynamicPluginRow {
  pluginId: string
  agentId: string
  packages: DynamicPackage[]
  currentPackageId?: string
  nextPackageId?: string
  activeRun?: { pluginRunId: string; packageId: string }
  latestRun?: DynamicRunAttempt
}

export const fetchDynamicInventory = (): Promise<DynamicPluginRow[]> =>
  apiCall('dynamicCordisRunner/inventory', {})

// ---- 轮询 composable ----

/** 固定间隔轮询；页面不可见时跳过，回到可见立即补一次。 */
export function usePolling(loader: () => Promise<unknown>, intervalMs = 3000): void {
  let timer: number | undefined
  const tick = (): void => {
    if (document.visibilityState === 'visible') void loader()
  }
  const onVisible = (): void => {
    if (document.visibilityState === 'visible') void loader()
  }
  onMounted(() => {
    void loader()
    timer = window.setInterval(tick, intervalMs)
    document.addEventListener('visibilitychange', onVisible)
  })
  onUnmounted(() => {
    window.clearInterval(timer)
    document.removeEventListener('visibilitychange', onVisible)
  })
}

// ---- 启用/禁用（patch 托管行）----

export interface PendingIntent {
  intent: boolean
  deadline: number
}

/** enabled 是 UI 语义；patch 层写的是 disabled 标志，取反。 */
export const setPluginEnabled = (entryId: string, enabled: boolean): Promise<void> =>
  invoke('plugin_set_enabled', { entryId, disabled: !enabled })

// ---- 安装/移除任务 ----

export interface PluginTaskView {
  taskId: string
  kind: 'install' | 'remove'
  spec: string
  status: 'running' | 'done' | 'failed'
  outputTail: string[]
  exitCode: number | null
}

export const installPlugin = (spec: string): Promise<string> => invoke('plugin_install', { spec })

export const removePlugin = (spec: string): Promise<string> => invoke('plugin_remove', { spec })

// ---- 设置（settings.*，静态方法不带 args 壳）----

export interface SecretSlot {
  path: string[]
  set: boolean
}

export interface SettingsNamespaceView {
  ns: string
  schema: unknown
  value: unknown
  base?: unknown
  user?: unknown
  applies: 'live' | 'restart'
  secrets: SecretSlot[]
  revision: number
}

export interface SettingsDescription {
  writable: boolean
  hasDocument: boolean
  namespaces: SettingsNamespaceView[]
}

export const describeSettings = (): Promise<SettingsDescription> => apiCall('settings.describe', {})

export const updateSettings = (ns: string, patch: unknown, expectedRevision?: number): Promise<unknown> =>
  apiCall('settings.update', { ns, patch, expectedRevision })

export type SettingsOp = { op: 'set'; path: string[]; value: unknown } | { op: 'unset'; path: string[] }

export const mutateSettings = (ns: string, ops: SettingsOp[], expectedRevision?: number): Promise<unknown> =>
  apiCall('settings.mutate', { ns, ops, expectedRevision })

// ---- 动态插件操作（回执是 value 而非信封错误，前端自行判 ok）----

export type DynamicStopReceipt = { ok: true } | { ok: false; reason: string; message?: string }

export const stopDynamicPlugin = (agentId: string, pluginId: string): Promise<DynamicStopReceipt> =>
  apiCall('dynamicCordisRunner/stopFromPanel', { agentId, pluginId })

export type DynamicUndefineReceipt =
  | { ok: true; wasRunning: boolean }
  | { ok: false; reason: string; message?: string }

export const undefineDynamicPlugin = (agentId: string, pluginId: string): Promise<DynamicUndefineReceipt> =>
  apiCall('dynamicCordisRunner/undefineFromPanel', { agentId, pluginId })
