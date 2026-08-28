# deepseek-harness-client 架构

> 基于 Tauri 2 的 DeepSeek Harness（`dsh`）桌面客户端。
> 设计原则：Unix/Linux 设计哲学（Eric Raymond《Unix 编程艺术》与 Linux 内核设计惯例）。
> 本文档与项目设计库中的记录（design id `204add95`）同源，随实现推进更新。

## 0. 事实基础（源自上游 H:\code\deepseek-harness 源码勘察）

- `dsh web` 启动 loopback HTTP+WS 宿主；线上契约：`POST /api/<method>`（一元 RPC）、`POST /api/respond`（应答审批/提问）、`WS /api/events.mux|host`（下行事件流）。
- 契约代码（`packages/host/apiproxy/src/api/`）浏览器安全、零 Node 依赖，官方明确预留"独立发布客户端"的位置。
- **就绪契约**：stdout 打印 `dsh web: http://127.0.0.1:<port>` 时 `/api` 必已挂载（上游注释：*"The URL line is a readiness signal: supervisors RPC as soon as they observe it"*）。本客户端的端口发现与就绪判定完全依赖这一行，不自造探测。
- 事实源是 append-only 的 `SessionEvent` 日志（"Model-visible means logged"），UI 只是流的投影。
- 信任栅栏：`/api` 仅接受 loopback Host 头；特权方法（`settings.*`、`credentials.*` 等）永远钉死 loopback。

## 1. 三平面总体架构

```
┌────────────────────────────────────────────────────┐
│ 客户端平面 (Tauri WebView, Vue 3 + TS)               │
│   UI 特性 ← 运行时存储 ← 连接控制器 ← tauri 传输适配  │
├──────────── invoke(RPC) ▲ Tauri events(流) ─────────┤
│ 宿主平面 (Rust, 只提供机制不做策略)                    │
│   薄命令层 · dsh-proxy · dsh-supervisor · dsh-bridge │
├──────────── 子进程 spawn ▲ loopback HTTP/WS ─────────┤
│ 内核平面 (dsh 守护进程, 不修改)                        │
│   Cordis 插件树 · append-only SessionEvent 日志      │
└────────────────────────────────────────────────────┘
```

Linux 隐喻：dsh 守护进程 = 内核空间；`dsh-supervisor` = init（PID 1）式进程监督；命名事件流 `daemon://*`、（P1 的）`dsh://stream/mux|host` = 设备文件；SessionEvent → 代理 → 存储 → UI 投影 = 管道；个位数 Tauri 命令 = 小而稳定的系统调用表；守护进程 stdout/stderr 环形缓冲 + 诊断视图 = syslog。

## 2. Unix 哲学 → 决策映射

| 原则 | 决策 |
|---|---|
| 模块原则：只做一件事并做好 | 三个 Rust crate 各司其职；dsh 内核零修改 |
| 组合原则：设计与端口相连 | 前端 transport 可替换（直连 / Tauri IPC / 内存测试载体） |
| 表示原则：知识捆入数据 | UI 不持有事实，是 SessionEvent 流的投影 |
| 机制与策略分离 | Rust 只做机制；审批/权限/模型选择留在 dsh |
| 简洁原则（Worse is better） | P0 直接内嵌上游 Web UI，自有 UI 增量替换 |
| 透明原则 | 日志尾、重启计数、解析出的启动命令全部可见 |
| 健壮原则 | 退避重启；Job Object/进程组级联终止；会话日志天然持久 |
| 经济原则 | 复用上游 wire 契约与 MIT 代码，不自造协议 |
| 最少惊讶 | 沿用 dsh 词汇（session、mux 帧、approval rpcId） |
| 保护原则 | Tauri capability 最小化；一切经 loopback；凭据零落 WebView |

## 3. P0 已实现范围（本文档随阶段演进）

### 进程模型与生命周期

1. **启动**：Tauri main → `dsh-supervisor` 以 `--port 0` spawn `dsh web`（解析顺序：`DSH_CLIENT_BIN` → PATH 上的 `dsh` → `npx -y @deepseek-ai/dsh@latest`）。Windows 下经 `cmd /c` 执行 npm shim 并设 `CREATE_NO_WINDOW`；子进程置于 kill-on-close Job Object（Unix：独立进程组），父进程退出即级联终止。
2. **前置校验（preflight）**：在 `Supervisor::start` 之前由 `dsh-supervisor::preflight::run_probe` 探测"实际将跑 dsh 的 Node 运行时"（优先 `DSH_CLIENT_BIN` 第一 token，回退到 PATH 上 `node`），调用 `node -p "process.version"` 读取版本并与上游 `engines.node = "^22.19.0 || >=24.0.0"` 比对；同步检查 `dsh`/`npx` 是否在 PATH 上。不达标则**不启动 supervisor**，emit `preflight://report` 事件，webview 留在启动页引导视图。重新探测走 `preflight_check` 命令，不现场启动 supervisor（用户需完全退出并重启应用）。
3. **就绪**：监督者逐行读 stdout，匹配 `dsh web: http://127.0.0.1:<port>` → 发 `Ready{port, pid}` → 应用层把 webview 导航至 `http://127.0.0.1:<port>`（上游 Web UI 同源加载，fetch/WS 原生可用）。
4. **运行**：异常退出 → 指数退避（500ms·2ⁿ，上限 30s）重启；`daemon_restart` 立即重启并重置退避；崩溃时若 webview 正停在守护进程页，则导航回本地启动页（origin 对比，避免重启页闪动）。
5. **关停**：`ExitRequested` → 监督者停机（取走 kill 句柄 → Job 关闭/`killpg` → 5s 宽限 → 强杀）。

### 事件与命令面（P0 的"系统调用表"）

| 事件 | 载荷 |
|---|---|
| `daemon://starting` | `{attempt}` |
| `daemon://ready` | `{port, pid}` |
| `daemon://crashed` | `{attempt, reason, retryInMs}` |
| `daemon://stopped` | `{}` |
| `daemon://log` | `{stream, line}` |
| `preflight://report` | `{engineOk, version, versionSource, nodePath, required, dshReachable, failure, dshCommandDisplay}` |

命令：`daemon_status`、`daemon_log_tail(max)`、`daemon_restart`、`daemon_stop`、`preflight_check`（返回同上 `preflight://report` 载荷）。

### 启动页（客户端平面 P0）

状态机 `starting / running / backoff / stopped`：监听 `daemon://*` 五通道 + 挂载时 `daemon_status` 兜底（消除事件竞态）；展示守护进程日志尾（stdout 常态 / stderr 红）、解析出的启动命令、失败原因与重试倒计时，提供"立即重试"。

### 代码组织

```
src-tauri/
├── crates/dsh-supervisor/   # P0：spawn / URL 行就绪 / 退避 / Job Object / 日志环
│   └── src/{lib,resolve,preflight,winjob}.rs
└── src/{main,lib}.rs        # 薄命令层 + 事件转发 + 导航（P1 起 dsh-proxy 在此生长）
src/                         # Vue 3 启动页（P2 起长出自有 UI）
```

依赖方向单向；`dsh-supervisor` 不依赖 tauri，可独立测试。

## 4. 后续阶段

- **P1 机制完备**：`dsh-proxy`（loopback HTTP 中继 + WS 帧扇出为 `dsh://stream/*`）、`transport-tauri`、`host.describe` 版本握手（feature-detect 软降级）、诊断页、单实例。
- **P2 自有 UI**：Vue 客户端平面 —— 会话流、审批卡、工具渲染、设置，全部渲染为 SessionEvent 流的投影。
- **P3 桌面化**：托盘、系统通知、深链、打包 Node sidecar（免系统 Node 分发）、自动更新与版本锁定清单。

## 5. 已识别风险

- 上游 developer preview，契约会破坏性变更 → P1 握手缓解。
- Windows `npx` 冷启动慢 → 启动页进度反馈；P3 打包 sidecar 根治。
- `DSH_CLIENT_BIN` 含空格路径时经 `cmd /c` 的引号处理有限 → 文档注明。

## 6. 插件管理（首个 syscall 表扩展实例）

插件管理是"系统调用表"自 P0 之后的第一个扩展实例：命令面新增 `dsh_api_call`（通用 RPC 透传）、`plugin_set_enabled`（写 patch 托管行）、`plugin_install`/`plugin_remove`（入 CLI 任务队列）、`open_plugins_window`；事件面新增 `plugins://task`。新增的仍是薄机制命令，不含策略。

- **`dsh-bridge` crate**：daemon `/api` 信封的客户端半。上游信任栅栏只认 loopback Host，webview 过不去，因此一切 API 都经 Rust 转发到 loopback——机制层，策略留在 dsh。
- **`dsh-profile` crate**：`cordis.patch.yml` 的文本级托管行读写（绝不 YAML 解析，`!!js` 之类标签天然安全）+ `dsh plugin` CLI 的串行任务队列（pnpm profile 锁，同一时刻只跑一个任务）。
- **窗口拓扑**：`main`（splash → 上游 Web UI）与 `plugins`（管理面，常驻隐藏）两个窗口；按窗口 label 选根组件（`src/main.ts`），无 router。

已知边界（明确不做 / 后续迭代）见 spec §9（`docs/superpowers/specs/2026-08-27-plugin-management-design.md`）。
