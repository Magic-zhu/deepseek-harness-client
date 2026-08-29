# IPC 契约

Tauri 2 命令面与事件面的完整参考。所有载荷均以 **camelCase** JSON 在 WebView ↔ Rust 之间传递（Rust 侧 `#[serde(rename_all = "camelCase")]`）。

调用方向：

- WebView → Rust：`invoke('cmd', args)`（`@tauri-apps/api/core` 的 `invoke`）
- Rust → WebView：`app.emit("channel://name", payload)`（监听：`listen("channel://name", handler)`）

跨窗口通信（无 router）走 `localStorage` 事件转发：写侧 `localStorage.setItem('dsh.ipc', JSON.stringify({...}))` 后派发 `storage` 事件，读侧监听该事件并解析。

---

## 命令面

### 守护进程（`dsh-supervisor` 代理）

| 命令 | 入参 | 返回 | 备注 |
|---|---|---|---|
| `daemon_status` | — | [`DaemonStatus`](#daemonstatus) | 当前 supervisor 快照 |
| `daemon_log_tail` | `{ max?: number }` | `LogLine[]` | `max` 默认 60，clamp 到 1..=500 |
| `daemon_restart` | — | `void` | 立即重启并重置退避 |
| `daemon_stop` | — | `void` | 停止 supervisor（应用不退出） |
| `preflight_check` | — | [`PreflightReport`](#preflightreport) | 重新探测 Node / dsh 可用性 |

#### `DaemonStatus`

```ts
interface DaemonStatus {
  state: 'starting' | 'running' | 'backoff' | 'stopped'
  pid: number | null
  port: number | null
  attempt: number
  restarts: number
  lastError: string | null
  command: string         // 解析出的启动命令（仅用于诊断展示）
}
```

#### `PreflightReport`

```ts
interface PreflightReport {
  engineOk: boolean
  version: string | null
  versionSource: 'overrideBin' | 'pathNode' | 'unavailable'
  nodePath: string | null
  required: string        // 当前固定的 ^22.19.0 || >=24.0.0
  dshReachable: boolean
  failure: string | null
  dshCommandDisplay: string
}
```

### `dsh` `/api` 透传

| 命令 | 入参 | 返回 | 备注 |
|---|---|---|---|
| `dsh_api_call` | `{ method: string, payload: unknown }` | `unknown` | WebView 跨不过 loopback trust fence，故统一经 Rust 转发。错误以 `[code] message` 字符串 reject |

业务错误（`settings.*`、`credentials.*` 等）以 `[code] message` 前缀 reject，前端按前缀模式匹配；客户端内部错误直接 reject 字符串。

### 插件管理

| 命令 | 入参 | 返回 | 备注 |
|---|---|---|---|
| `plugin_install` | `{ spec: string }` | `string` | 入串行任务队列，返回 task ID |
| `plugin_remove` | `{ spec: string }` | `string` | 入串行任务队列，返回 task ID |
| `plugin_tasks_list` | — | [`PluginTaskView[]`](#plugintaskview) | 当前所有任务快照（队列 / 运行 / 已完成） |
| `plugin_set_enabled` | `{ entryId: string, disabled: boolean }` | `void` | 写 `cordis.patch.yml` 托管行 |

`spec` 是单个 argv token；Windows 经 `cmd /c` 转义，空格与 `&|<>^"` 等元字符被拒绝。

#### `PluginTaskView`

```ts
interface PluginTaskView {
  taskId: string
  kind: 'install' | 'remove'
  spec: string
  status: 'running' | 'done' | 'failed'
  outputTail: string[]    // 最近 ≤ 500 行
  exitCode: number | null
}
```

### 窗口与命令面板

| 命令 | 入参 | 返回 | 备注 |
|---|---|---|---|
| `open_plugins_window` | — | `void` | 显示并聚焦 `plugins` 窗口 |
| `show_main_window` | — | `void` | 显示并聚焦 `main` 窗口 |
| `open_palette_window` | — | `void` | 显示并聚焦 `palette` 窗口，同时 emit `palette://open` |
| `app_quit` | — | `void` | 经 `app.exit(0)` 干净停机 |

`open_plugins_window` / `show_main_window` 内部先 `hide()` 命令面板（避免 Windows 上透明无边框窗口在 z-order 残留导致焦点抢回）。

---

## 事件面

### `daemon://*` —— 守护进程生命周期

| 通道 | 载荷 | 时机 |
|---|---|---|
| `daemon://starting` | `{ attempt: number }` | supervisor 启动一次 spawn（attempt 从 1 计） |
| `daemon://ready` | `{ port: number, pid: number }` | stdout 出现 `dsh web: http://127.0.0.1:<port>` |
| `daemon://crashed` | `{ attempt: number, reason: string, retryInMs: number }` | 子进程异常退出，下一次尝试在 `retryInMs` 之后 |
| `daemon://stopped` | `{}` | supervisor 主动停机（请求退出） |
| `daemon://log` | `{ stream: 'stdout' \| 'stderr', line: string }` | 子进程一行 stdout / stderr |

主窗口收到 `daemon://ready` 后导航到 `http://127.0.0.1:<port>`；收到 `daemon://crashed` / `daemon://stopped` 且当前 origin 不是 `tauri://localhost` 时回到启动页。

### `preflight://report` —— 前置校验结果

| 通道 | 载荷 | 时机 |
|---|---|---|
| `preflight://report` | [`PreflightReport`](#preflightreport) | 应用启动时若 preflight 未通过，emit 一次；不通过的 WebView 留在启动页 |

启动页 mount 时主动 `invoke('preflight_check')` 兜底，消除事件竞态。

### `plugins://task` —— 插件任务更新

| 通道 | 载荷 | 时机 |
|---|---|---|
| `plugins://task` | [`PluginTaskView`](#plugintaskview) | 任务状态变化、output tail 更新 |

完整视图事件（每次 emit 都含完整当前任务），所以中间状态丢失无害；监听端按 `taskId` 合并即可。

### `palette://open` —— 命令面板重置

| 通道 | 载荷 | 时机 |
|---|---|---|
| `palette://open` | `{}` | `open_palette_window` / 托盘左键 / 全局快捷键 `Ctrl+Shift+P` 触发 |

命令面板 WebView 监听该事件以清空输入、聚焦。

---

## 客户端平面状态机

主窗口 splash 状态机：

```
   preflight://report(engineOk: false)
                  │
                  ▼
              ┌──────────┐    daemon://ready      ┌──────────┐
              │ SPLASH   │ ────────────────────▶ │  WEB UI  │
              │ 引导页    │                        └──────────┘
              └──────────┘                                ▲
                  ▲                                       │
   preflight://report(engineOk: true)                    │
                  │                                       │
   daemon://starting                                      │
                  ▼                                       │
              ┌──────────┐   daemon://ready / log         │
              │ STARTING │ ──────────────────────────────▶│
              └──────────┘                                │
                  │ daemon://crashed (retryInMs > 0)      │
                  ▼                                       │
              ┌──────────┐                                │
              │ BACKOFF  │ ──────────────────────────────▶│
              └──────────┘                                │
                  │ daemon://stopped                      │
                  ▼                                       │
              ┌──────────┐                                │
              │ STOPPED  │                                │
              └──────────┘
```

启动页 mount 时 `invoke('daemon_status')` 拿当前快照 + `listen` 订阅 5 通道 + `listen('preflight://report')`，三者并联保证不会因事件竞态停留在错状态。

---

## 错误约定

- 守护进程未就绪时调用 `dsh_api_call` → reject `daemon 未就绪：尚无可用端口（启动中或已崩溃）`
- preflight 失败时调用 `dsh_api_call` → reject `daemon 未启动（preflight 未通过），请先解决运行环境问题`
- `plugin_set_enabled` 找不到 home → reject `无法定位 dsh home（DSH_HOME 未设置且无法解析用户目录）`
- `plugin_install` / `plugin_remove` spec 非法 → reject `spec 为空或含首尾空白` / `spec 含非法字符：<spec>`
- 业务错误（经 `dsh_api_call`） → `[code] message` 字符串 reject

---

## 类型导出

前端约定：

- `src/daemon.ts` — 守护进程状态、事件、preflight 的 TS 类型与调用封装
- `src/palette.ts` — 命令面板的 TS 类型与调用封装
- `src/plugins/plugins.ts` — 插件管理面（inventory / dynamic / settings / tasks）的 TS 类型与调用封装