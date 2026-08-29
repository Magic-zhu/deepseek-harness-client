# 插件管理子系统

`plugins` 窗口（label = `plugins`，标题「插件管理」）是 `dsh` 插件的图形化管理面。它是宿主平面「系统调用表」自基本监督之上的第一个扩展实例。

设计原则仍照 [architecture.md §2](./architecture.md#2-unix-哲学--决策映射)：只提供机制，不持有策略。

---

## 三个面

### 1. Loader 插件清单

通过 `dsh_api_call('pluginInventory/list', {})` 拉取当前进程内已注册的 Cordis 插件条目。每条目：

```ts
interface PluginEntry {
  entryId: string
  moduleName: string
  enabled: boolean
  fiberPhase: 'pending' | 'loading' | 'active' | 'failed' | 'unloading' | null
}
```

启用 / 禁用通过 `plugin_set_enabled(entryId, disabled)` 写 `cordis.patch.yml` 托管行（**不**改 YAML 解析，安全处理 `!!js` 等标签）：

```ts
setPluginEnabled(entryId, enabled)  // UI 语义 = enabled；patch 层写 disabled = !enabled
```

热生效：`dsh` 监听 patch 文件变化，HMR 重启条目。前端验证靠 3 s 间隔轮询（`usePolling`，页面不可见时跳过）。

### 2. 动态插件

通过 `dynamicCordisRunner/*` 家族调用管理在运行期创建 / 卸载的插件：

| 调用 | 用途 |
|---|---|
| `dynamicCordisRunner/inventory` | 拉取所有动态插件行（包含每行的 packages / activeRun / latestRun） |
| `dynamicCordisRunner/stopFromPanel({ agentId, pluginId })` | 停掉单个插件实例（返回 receipt） |
| `dynamicCordisRunner/undefineFromPanel({ agentId, pluginId })` | 卸载定义 |

返回是 `{ ok: true, ... } | { ok: false, reason, message? }` 的 receipt 结构，而非 envelope 错误——业务方自行判 `ok` 字段。

`CordisRunStatus` 全集：`awaiting-approval` / `starting-host` / `client-pending` / `running` / `waiting` / `rejected` / `failed` / `cancelled` / `stopped`。需要审批时，前端弹出审批卡片（与上游 `respond` 协议一致），通过 `dsh_api_call('respond', {...})` 应答。

### 3. 安装 / 卸载任务队列

通过 `dsh plugin install <spec>` / `dsh plugin remove <spec>` CLI 子命令执行。CLI 不与守护进程 `/api` 直接交互，只读写 profile 目录，所以独立于 supervisor，可与 preflight 失败状态并存。

CLI 串行化：`dsh-profile::tasks::PluginTaskRunner` 在 tokio runtime 上起一个 worker，`mpsc::unbounded_channel` 收作业，同一时刻只跑一个任务，避免 `cordis.patch.yml` 与 profile 目录读写竞争。

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

完整视图事件经 `plugins://task` 通道广播（`broadcast::channel(64)`），丢失中间状态无害；窗口订阅并按 `taskId` 合并即可。

最近 32 个已完成任务保留为快照，新挂载的任务中心通过 `plugin_tasks_list` 一次性 catch up。

### 设置（settings）

`settings.describe` / `settings.update` / `settings.mutate` 均经 `dsh_api_call` 透传。命名空间视图：

```ts
interface SettingsNamespaceView {
  ns: string
  schema: unknown
  value: unknown
  base?: unknown
  user?: unknown
  applies: 'live' | 'restart'   // 是否需要重启 dsh
  secrets: SecretSlot[]         // 已设置的密钥条目（不返回值，仅 path / set）
  revision: number              // 用于乐观并发
}
```

更新通过 `settings.update(ns, patch, expectedRevision?)` 或 `settings.mutate(ns, ops, expectedRevision?)`：`expectedRevision` 不匹配时 `dsh` 拒绝，前端按返回值判 ok / retry。

---

## Spec 校验

`spec` 是单个 argv token，必须满足：

- 非空、无首尾空白；
- 不含 ASCII whitespace、`&|<>^"\r\n`。

Windows 下任务经 `cmd /c` 转义——不严格校验会被 shell 解析错误执行。元字符拒绝是为了在这一层先于 shell 拦截。

---

## 子系统拓扑

```
                            dsh /api (loopback)
                                 ▲
                                 │ dsh_api_call (Tauri invoke → dsh-bridge → HTTP)
                                 │
       ┌──────────┐  invoke    ┌─┴───────────────────────────┐
       │ Plugins  │ ─────────▶ │ src-tauri/src/lib.rs        │
       │ window   │ ◀───────── │   薄命令层 + 事件转发       │
       └──────────┘  plugins://task
                                 │
                                 │  ├─ dsh-bridge     (RPC 透传)
                                 │  ├─ dsh-profile    (patch 文件 + 任务队列)
                                 │  └─ dsh-supervisor (当前未直接参与，但启动 daemon)
                                 │
                                 ▼
                  dsh web 守护进程 + ~/.dsh/profile/cordis.patch.yml
```

依赖方向：`dsh-profile` / `dsh-bridge` 不依赖 Tauri，可在没有 Tauri runtime 的测试中端到端跑。

---

## 已知边界

- `dsh` 处于开发者预览阶段，inventory / dynamic API 的形态可能破坏性变更。客户端仅作为机制层 —— 一旦上游接口调整，前端类型与调用名需同步。
- `cordis.patch.yml` 不做 YAML 解析（安全考虑 `!!js` 标签），只做文本级托管行读写；HMR 由上游托管。
- 安装 / 卸载走 CLI 而非 `/api` 是上游决定的边界；本客户端无法绕开。
- `setPluginEnabled` 是「写意图」，热生效由上游 HMR 完成。轮询是验证手段，非触发手段。