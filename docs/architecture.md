# 架构

Tauri 2 桌面客户端，对 [DeepSeek Harness](https://github.com/deepseek-ai) (`dsh`) 提供开箱即用的桌面外壳。本文说明其结构、生命周期、IPC 契约与安全模型，作为阅读源码前的导航与实现约束的来源。

设计取向：遵循 Eric Raymond《Unix 编程艺术》与 Linux 内核设计惯例——客户端只做「监督 + 装载」，不改 `dsh` 内核、不自造协议。

---

## 1. 三平面

```
┌────────────────────────────────────────────────────┐
│ 客户端平面 (Tauri WebView · Vue 3 + TS)              │
│   UI 特性 ← 运行时存储 ← 连接控制器 ← tauri 传输适配  │
├──────────── invoke(RPC) ▲ Tauri events(流) ─────────┤
│ 宿主平面 (Rust · 只提供机制不做策略)                  │
│   薄命令层 · dsh-bridge · dsh-supervisor · dsh-profile │
├──────────── 子进程 spawn ▲ loopback HTTP/WS ─────────┤
│ 内核平面 (dsh 守护进程 · 不修改)                       │
│   Cordis 插件树 · append-only SessionEvent 日志       │
└────────────────────────────────────────────────────┘
```

Linux 隐喻：

- `dsh` 守护进程 = 内核空间；
- `dsh-supervisor` = init（PID 1）式进程监督；
- 事件流 `daemon://*` / `preflight://report` / `plugins://task` = 设备文件；
- SessionEvent → 桥接 → 存储 → UI 投影 = 管道；
- 个位数 Tauri 命令 = 小而稳定的系统调用表；
- 守护进程 stdout/stderr 环形缓冲 + 诊断视图 = syslog。

## 2. Unix 哲学 → 决策映射

| 原则 | 决策 |
|---|---|
| 模块原则 | 三个 Rust crate 各司其职；`dsh` 内核零修改 |
| 组合原则 | 前端 transport 可替换（直连 / Tauri IPC / 内存测试载体） |
| 表示原则 | UI 不持有事实，是 SessionEvent 流的投影 |
| 机制与策略分离 | Rust 只做机制；审批/权限/模型选择留在 `dsh` |
| 简洁原则 | 直接内嵌上游 Web UI，自有 UI 增量替换 |
| 透明原则 | 日志尾、重启计数、解析出的启动命令全部可见 |
| 健壮原则 | 退避重启；Job Object / 进程组级联终止；会话日志天然持久 |
| 经济原则 | 复用上游 wire 契约与 MIT 代码，不自造协议 |
| 最少惊讶 | 沿用 `dsh` 词汇（session、mux 帧、approval rpcId） |
| 保护原则 | Tauri capability 最小化；一切经 loopback；凭据零落 WebView |

## 3. 进程模型与生命周期

### 3.1 启动

1. Tauri `main` → `dsh-supervisor` 以 `--port 0` spawn `dsh web`（解析顺序：`DSH_CLIENT_BIN` → PATH 上的 `dsh` → `npx -y @deepseek-ai/dsh@latest`）。
2. Windows 下经 `cmd /c` 执行 npm shim 并设 `CREATE_NO_WINDOW`；子进程置于 kill-on-close Job Object（Unix：独立进程组），父进程退出即级联终止。
3. 应用退出触发 `ExitRequested`：监督者停机（取走 kill 句柄 → Job 关闭 / `killpg` → 5 s 宽限 → 强杀）。

### 3.2 前置校验（preflight）

在 `Supervisor::start` 之前由 `dsh-supervisor::preflight::run_probe` 探测「实际将跑 `dsh` 的 Node 运行时」：

- 优先取 `DSH_CLIENT_BIN` 第一个 token（解析后的 Node 二进制）；
- 回退到 PATH 上第一个 `node`；
- 调用 `node -p "process.version"` 读取版本，与上游 `engines.node = "^22.19.0 || >=24.0.0"` 比对；
- 同步检查 `dsh` / `npx` 是否在 PATH 上。

不达标则**不启动 supervisor**，emit `preflight://report` 事件，WebView 留在启动页引导视图；`preflight_check` 命令用于重新探测，但**不会现场启动 supervisor**——用户需完全退出并重启应用。

### 3.3 就绪判定

监督者逐行读 stdout，匹配 `dsh web: http://127.0.0.1:<port>` → 发出 `Ready{port, pid}` → 应用层把 WebView 导航至 `http://127.0.0.1:<port>`（上游 Web UI 同源加载，fetch / WS 原生可用）。

> 上游契约：该 URL 行出现时 `/api` 必已挂载（上游源码注释：*"The URL line is a readiness signal: supervisors RPC as soon as they observe it"*）。客户端不就绪探测、不二次确认。

### 3.4 运行与退避

- 异常退出 → 指数退避（500 ms · 2ⁿ，上限 30 s）重启；
- `daemon_restart` 立即重启并重置退避；
- 崩溃时若 WebView 正停在守护进程页（origin 对比），则导航回本地启动页，避免重启页闪动。

`RestartPolicy::default()`：

| 字段 | 值 |
|---|---|
| `initial` | 500 ms |
| `multiplier` | 2.0 |
| `max` | 30 s |

延迟公式：`initial · multiplier^(attempt - 1)`，结果对 `max` 取最小值。

## 4. 宿主平面 crate 划分

| crate | 职责 | 关键导出 |
|---|---|---|
| `dsh-supervisor` | spawn / URL 行就绪 / 退避 / Job Object / 日志环 / preflight | `Supervisor`, `SupervisorEvent`, `State`, `Status`, `RestartPolicy`, `PreflightReport`, `run_probe` |
| `dsh-bridge` | `dsh` `/api` 信封的客户端半：构造 RPC body、解析响应、错误映射 | `ApiClient`, `ApiError`, `validate_method`, `wrap_payload`, `build_body`, `parse_response` |
| `dsh-profile` | `cordis.patch.yml` 文本级托管行读写 + `dsh plugin` CLI 串行任务队列 | `PluginTaskRunner`, `PluginTaskView`, `patch::set_disabled`, `home::*` |

依赖方向单向：`dsh-supervisor` / `dsh-bridge` / `dsh-profile` 不依赖 Tauri，可在没有 Tauri 运行时的情况下单元测试。

`src-tauri/src/lib.rs` 是薄装配层：注册 Tauri 命令、安装事件转发、安装托盘与全局快捷键，不持有策略。

## 5. 客户端平面窗口拓扑

`tauri.conf.json` 声明三个 WebView 窗口：

| label | 标题 | 可见性 | 备注 |
|---|---|---|---|
| `main` | DeepSeek Harness | 默认 | 主窗口：启动期渲染 splash → 就绪后导航到 `http://127.0.0.1:<port>` |
| `plugins` | 插件管理 | `visible: false` | 启动时隐藏，从托盘 / 命令面板唤起 |
| `palette` | 命令面板 | `visible: false`、`decorations: false`、`transparent: true`、`alwaysOnTop: true`、`skipTaskbar: true` | 透明无边框，悬浮在主窗口正中 |

入口 (`src/main.ts`) 按 `window.__TAURI_INTERNALS__.metadata.currentWindow.label` 选择根组件（`App.vue` / `PluginsApp.vue` / `PaletteApp.vue`），无 router。

## 6. 跨平面契约

### 6.1 命令面（Tauri `invoke`）

| 命令 | 用途 | 返回 |
|---|---|---|
| `daemon_status` | 当前 supervisor 状态 | `DaemonStatusDto` |
| `daemon_log_tail(max?)` | 守护进程日志尾（默认 60，clamp 1..=500） | `LogLineDto[]` |
| `daemon_restart` | 立即重启并重置退避 | `()` |
| `daemon_stop` | 停止 supervisor（不退出应用） | `()` |
| `preflight_check` | 重新探测 Node / dsh | `PreflightReport` + `dshCommandDisplay` |
| `dsh_api_call(method, payload)` | 通用 RPC 透传到 `dsh` `/api` | `serde_json::Value` |
| `plugin_install(spec)` | 入串行任务队列：`dsh plugin install <spec>` | 任务 ID |
| `plugin_remove(spec)` | 入串行任务队列：`dsh plugin remove <spec>` | 任务 ID |
| `plugin_tasks_list` | 当前所有任务快照（queueing / running / finished） | `PluginTaskView[]` |
| `plugin_set_enabled(entryId, disabled)` | 写 `cordis.patch.yml` 托管行 | `()` |
| `open_plugins_window` | 显示并聚焦 `plugins` 窗口 | `()` |
| `show_main_window` | 显示并聚焦 `main` 窗口 | `()` |
| `open_palette_window` | 显示并聚焦 `palette` 窗口 + emit `palette://open` | `()` |
| `app_quit` | `app.exit(0)`，经 `RunEvent::ExitRequested` 走干净停机 | `()` |

`dsh_api_call` 的 `Err` 字符串形如 `[code] message`——`settings.*` 等业务错误以此为前缀模式匹配。

### 6.2 事件面（WebView `listen`）

| 事件 | 载荷 | 来源 |
|---|---|---|
| `daemon://starting` | `{ attempt }` | `SupervisorEvent::Starting` |
| `daemon://ready` | `{ port, pid }` | `SupervisorEvent::Ready` |
| `daemon://crashed` | `{ attempt, reason, retryInMs }` | `SupervisorEvent::Crashed` |
| `daemon://stopped` | `{}` | `SupervisorEvent::Stopped` |
| `daemon://log` | `{ stream: "stdout" \| "stderr", line }` | `SupervisorEvent::Log` |
| `preflight://report` | `PreflightReport`（见下） | `run_probe` |
| `plugins://task` | `PluginTaskView` | `PluginTaskRunner` |
| `palette://open` | `()` | `open_palette`（命令面板重置提示） |

`PreflightReport` 字段：`engineOk`、`version`、`versionSource: "overrideBin" | "pathNode" | "unavailable"`、`nodePath`、`required`、`dshReachable`、`failure`。

### 6.3 客户端平面内部 IPC

`sync` API 在多 WebView 窗口间经 localStorage 事件转发（详见 [docs/ipc.md](ipc.md)）。

## 7. 安全模型

- **信任栅栏一致**：`dsh` `/api` 仅接受 loopback Host 头，特权方法（`settings.*`、`credentials.*` 等）永远钉死 loopback。客户端不绑定、不转发任何非本机地址。
- **WebView 无法满足 Origin / Sec-Fetch-Site**：所有 `/api` 调用必须经 Rust `dsh_api_call` 转发。
- **凭据零落 WebView**：loopback HTTP 是唯一通路，凭据在请求体内由 dsh 处理。
- **Tauri capability 最小化**：`capabilities/default.json` 仅启用 `core:default`（事件、窗口 API），三窗口共用同一能力集。
- **CSP**（`tauri.conf.json`）：

  ```
  default-src 'self';
  connect-src 'self' ipc: http://ipc.localhost http://127.0.0.1:* ws://127.0.0.1:*;
  style-src 'self' 'unsafe-inline';
  img-src 'self' data:
  ```

- **进程隔离**：守护进程置于独立进程组 / Job Object；自身 crash 不会污染宿主进程。

## 8. 桌面集成

- **托盘**：托盘图标右键菜单可调起命令面板 / 插件窗口 / 主窗口、重启守护进程、退出应用；左键单击直接打开命令面板。托盘菜单 ID 与命令名一一对应（`show_palette` / `show_plugins` / `show_main` / `restart` / `quit`）。
- **全局快捷键**：`Ctrl+Shift+P` 注册为命令面板的全局快捷键（`tauri-plugin-global-shortcut`）。必须放全局而非 WebView keydown 监听：主窗口会导航到上游 Web UI（host `127.0.0.1:<port>`），Windows / WebView2 在 host 层把 `Ctrl+Shift+P` 绑为打印预览，`keydown` 永远到不了 JS 层。
- **窗口关闭 = 最小化到托盘**：`RunEvent::ExitRequested` 在 `code.is_none()` 时 `prevent_exit()` 并 `hide()` 主窗口；显式退出（托盘菜单 / `app_quit`）才走干净停机。
- **命令面板居中**：每次 `open_palette` 都重新计算 `palette` 在 `main` 中心的物理像素位置；窗口声明期 `center: true` 只生效一次，之后用户拖动后位置会漂移，必须每次重算。

## 9. 已知边界与已识别风险

- 上游处于开发者预览阶段，wire 契约会破坏性变更 → 通过 `host.describe` 握手 + feature-detect 软降级缓解。
- Windows `npx` 冷启动慢 → 启动页给出进度反馈；根治手段是把 Node 与 `dsh` 一起打进 sidecar 分发。
- `DSH_CLIENT_BIN` 含空格路径时经 `cmd /c` 的引号处理有限 → 文档建议优先使用 PATH 上的 `dsh`。
- 关闭主窗口 ≠ 退出应用（最小化到托盘）。退出只能从托盘菜单「退出」或命令面板 `app_quit` 触发，避免误关导致守护进程一起死。
- Tauri 命令 `Option<State>` 在 Tauri 2 不是合法命令参数（`Option` impl 仅用于前端可选参数），故 `dsh_api_call` 用 `AppHandle::try_state` 探测，preflight 失败时给出可读错误。

## 10. 进一步阅读

- [docs/ipc.md](ipc.md) — 完整 Tauri 命令与事件参考（含载荷 schema）
- [docs/plugin-management.md](plugin-management.md) — 插件管理子系统设计
- [docs/windows-shortcuts.md](windows-shortcuts.md) — 窗口、全局快捷键与托盘的行为说明