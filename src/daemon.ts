import { invoke } from '@tauri-apps/api/core'

export type DaemonState = 'starting' | 'running' | 'backoff' | 'stopped'
export type LogStream = 'stdout' | 'stderr'

/** `daemon_status` 命令的返回结构（Rust 侧 serde camelCase）。 */
export interface DaemonStatus {
  state: DaemonState
  pid: number | null
  port: number | null
  attempt: number
  restarts: number
  lastError: string | null
  command: string
}

export interface StartingEvent { type: 'starting'; attempt: number }
export interface ReadyEvent { type: 'ready'; port: number; pid: number }
export interface CrashedEvent { type: 'crashed'; attempt: number; reason: string; retryInMs: number }
export interface StoppedEvent { type: 'stopped' }
export interface LogEvent { type: 'log'; stream: LogStream; line: string }

export type DaemonEvent =
  | StartingEvent
  | ReadyEvent
  | CrashedEvent
  | StoppedEvent
  | LogEvent

/** daemon://* 事件通道全集。 */
export const DAEMON_CHANNELS = [
  'daemon://starting',
  'daemon://ready',
  'daemon://crashed',
  'daemon://stopped',
  'daemon://log',
] as const

export interface LogLine {
  stream: LogStream
  line: string
}

export const fetchStatus = (): Promise<DaemonStatus> => invoke('daemon_status')

export const fetchLogTail = (max = 60): Promise<LogLine[]> => invoke('daemon_log_tail', { max })

export const restartDaemon = (): Promise<void> => invoke('daemon_restart')

/** Rust 端 `PreflightReportDto` 扁平化后的字段（带 `dshCommandDisplay`）。 */
export interface PreflightReport {
  engineOk: boolean
  version: string | null
  versionSource: 'overrideBin' | 'pathNode' | 'unavailable'
  nodePath: string | null
  required: string
  dshReachable: boolean
  failure: string | null
  dshCommandDisplay: string
}

/** `preflight://*` 事件通道全集。 */
export const PREFLIGHT_CHANNELS = ['preflight://report'] as const

/** `preflight_check` 命令的包装：仅重新探测 Node 版本，不启动 supervisor。 */
export const fetchPreflight = (): Promise<PreflightReport> => invoke('preflight_check')
