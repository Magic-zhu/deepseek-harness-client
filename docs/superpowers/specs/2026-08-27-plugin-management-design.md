# 插件管理功能设计（@deepseek-ai/dsh plugin management）

日期：2026-08-27
状态：已获用户批准（brainstorming architectural 路径）
任务记录：vibercoding task `9bca1491-d344-433a-a71b-19dc6f0e0120`

## 1. 背景与目标

dsh 的 Cordis 架构里"一切都是插件"。上游 Web UI 只提供只读插件清单页签与设置页签，刻意不暴露任何启停/安装/卸载的 RPC。本功能在客户端中提供**完整生命周期**的插件管理：清单 + 状态、启用/禁用、安装/移除、每插件设置编辑，以及模型运行时定义的动态插件的查看/停止/删除。

UI 落点：**独立管理窗口**（主窗口行为不变，daemon ready 后照常跳转上游 Web UI）。

### 范围确认（用户 2026-08-27 答复）

- 管理范围：完整生命周期（清单 + 设置 + 启停 + 装卸）
- 动态插件：纳入管理（展示 + 停止 + 删除）
- UI：独立 Tauri 窗口，splash 页加入口按钮

## 2. 上游 ground truth（已从源码钉死，实现时以此为准）

上游源码位置：`H:\code\deepseek-harness`（pnpm monorepo）。

### 2.1 wire 信封（静态方法与 Typert Remote 共用）

`packages/host/apiproxy/src/api/rpc.ts`：

- 请求：`POST http://127.0.0.1:<port>/api/<method>`，JSON body
  `{ "type": "client-request", "rpcId": "<uuid>", "method": "<method>", "payload": <object> }`
- 响应 body：`{ "type": "server-response", "rpcId": "<回声>", "result": { "ok": true, "value"? } | { "ok": false, "error": { "code", "message", "details" } } }`
- rpcId 由调用方生成，响应回声必须一致；业务错误永远走 `result.error`，不走 HTTP 状态码（非 2xx 仅为传输层失败）。
- Typert Remote 端点（`packages/api/gateway/src/index.ts`）：method 形状为 `<namespace>/<method>`（如 `pluginInventory/list`），payload **必须恰好是一个 `{ "args": { ... } }` 字段**；void 结果的响应**没有** `value` 字段。

### 2.2 信任栅栏（`packages/client/connection/src/api-request-trust.ts`）

- Host 头必须是 loopback（或部署声明的 trustedHost）——reqwest 自动带 `127.0.0.1:<port>` ✓
- `Sec-Fetch-Site: cross-site` 一律拒绝；带 Origin 时 Origin host 必须等于 Host authority——**浏览器/webview 直连必死，Rust 客户端不带这些头，天然通过** ✓
- `settings.*` 等特权方法 loopback-pinned：Rust 从本机发起即满足 ✓
- 结论：所有 daemon API 调用必须经 Rust 转发，前端永远不直连、不需要知道端口。

### 2.3 插件清单（只读，无推送）

`pluginInventory/list`（`packages/host/plugin-inventory/src/types.ts`），args `{}`，返回：

```ts
{ entries: Array<{
    entryId: string          // loader 树 id，嵌套为 ':' 连接
    moduleName: string       // import  specifier 原文
    enabled: boolean         // 含祖先 group 的有效启用态
    fiberPhase: 'pending'|'loading'|'active'|'failed'|'unloading'|null  // null = 无活 fiber
}> }
```

group 行不出现；**无变更事件，客户端轮询**。

### 2.4 设置（`settings.*`，特权，loopback-pinned）

`packages/host/apiproxy/src/api/settings.ts`：

- `settings.describe {}` → `{ writable, hasDocument, namespaces: [{ ns, schema, value, base?, user?, applies: 'live'|'restart', secrets: [{path: string[], set: boolean}], revision }] }`
  - `schema` 是 schemastery `schema.toJSON()` 序列化；`value`/`base`/`user` 均已脱敏（secret 字段永不出现）；`revision` 用于乐观并发。
- `settings.update { ns, patch, expectedRevision? }` → merge 进 user 层；secret 可写入（write-only）。
- `settings.mutate { ns, ops: [{op:'set',path,value}|{op:'unset',path}], expectedRevision? }` → 路径级增删（删除 secret 的安全路径）。
- `settings.replace { ns, section, expectedRevision? }` → 整体重置（`{}` = 恢复默认）。
- 错误码：`settings-rejected`（schema/存储拒绝）、`settings-conflict { ns, expected, actual }`（revision 过期）。

### 2.5 启用/禁用：patch 层语义（`vendor/include/src/index.ts`）

- profile 组合 = bundle patch 层 → 用户层 `$DSH_HOME/profiles/<name>/cordis.patch.yml` → `--patch` 覆盖；**后写赢**。
- patch 文件是 YAML list：`- insert: [entry...]` 插入新行；`- id: <entryId>` + 覆盖键逐键覆盖目标行（`config` 是整体替换，`disabled` 仅置标志位）。
- 文件含 `!!js` 表达式方言——**Rust 侧绝不整体 YAML 解析/求值**，只做文本级操作。
- HMR（`cordis-plugin-hmr` + app-boot `watchUserPatches`）监听用户 patch 文件并热生效；**package.json 的 bundle 层变更不热生效，需重启 daemon**。
- 边界：嵌套 entry（`:` 连接 id）可能无法被 patch 定址；操作后以轮询验证生效，失败如实报错。

### 2.6 安装/移除：`dsh plugin` CLI（`apps/cli/src/plugin.ts`）

- `dsh plugin --profile <name> <pnpm args...>`：初始化 profile（首用）→ 在 profile 目录跑 pnpm →  reconcile `dsh.profile.bundles`。
- pnpm 不在 PATH 时退出码 127；pnpm 失败时 stderr 有明确诊断（含 git 依赖 prepare 脚本被 pnpm≥10 拦的提示）。
- 本客户端 daemon 用的 profile 固定为 `web`（`dsh web` ≡ `--profile web`）。
- 从 Rust spawn 时：piped stdout/stderr（非 inherit）、Windows 下 `cmd /c` + `CREATE_NO_WINDOW`（与 supervisor 一致）、设 `CI=true` 保持 pnpm 非交互。

### 2.7 动态插件（`dynamicCordisRunner/*`，`packages/extensions/cordis-host-runner/src/types.ts`）

- `inventory {}` → `DynamicCordisInventoryRow[]`：`{ pluginId, agentId, packages: [{packageId,name,purpose,hasHostHalf,hasClientHalf}], currentPackageId?, nextPackageId?, activeRun?: {pluginRunId,packageId}, latestRun?: { pluginRunId, packageId, mode, status, approvalRequestId?, requiresApproval?, host: {status,waitingFor,error?}, client: {...}, error?: {phase,message,stack?} } }`
- `stopFromPanel { agentId, pluginId }` → `{ok:true} | {ok:false, reason:'plugin-missing'|'not-running', message}`
- `undefineFromPanel { agentId, pluginId }` → `{ok:true, wasRunning} | {ok:false, reason:'plugin-missing', message}`
- 动态插件会话级、进程内、daemon 重启即失——UI 必须标注。

### 2.8 dsh home 解析（`packages/util/home-paths/src/index.ts`）

`$DSH_HOME`（空串视为未设）→ `~/.dsh`。profile 目录：`<home>/profiles/web/`。客户端必须与 daemon 解析出同一个 home（daemon 是我们 spawn 的，继承同一环境）。

## 3. 架构

```
插件窗口 (Vue)                     Host plane (Rust)                   dsh daemon
┌──────────────┐   invoke       ┌─────────────────┐   HTTP POST    ┌──────────┐
│ PluginsApp   │ ─────────────► │ 薄命令层         │ ─────────────► │ /api/... │
│ (投影,不存事实)│ ◄───────────── │  dsh-bridge      │ 127.0.0.1:port │          │
└──────────────┘  plugins://*   │  dsh-profile     │ ─── 文件/pnpm ─►│ cordis.  │
                                │  (patch/CLI 任务) │                │ patch.yml│
                                └─────────────────┘                └──────────┘
```

### 3.1 新增 crate：`dsh-bridge`（wire 客户端，零 Tauri 依赖）

```rust
pub struct ApiClient { /* reqwest::Client, base http://127.0.0.1:<port> */ }
impl ApiClient {
    pub fn new(port: u16) -> Self;
    pub async fn call(&self, method: &str, payload: serde_json::Value)
        -> Result<serde_json::Value, ApiError>;  // void 结果返回 Value::Null
}
pub enum ApiError {
    Transport(String),                 // 连接失败/超时/非 2xx
    Rpc { code: String, message: String, details: serde_json::Value },  // 业务错误原样上抛
    Protocol(String),                  // 信封畸形 / rpcId 回声不符
}
```

- 负责：信封组包/解包、rpcId（uuid v4）、30s 超时、响应回声校验。
- **Typert 自动包装**：method 含 `/`（`<ns>/<name>`）时，bridge 自动把入参 payload 包成 `{ "args": payload }`；静态方法（`<domain>.<name>`）原样发送。前端永远不写 `args` 壳。
- method 白名单形状校验：仅允许 `<domain>.<name>` 或 `<ns>/<name>`（字母数字 `-` `_`），拒绝路径字符——防 webview 拼出怪异 URL。
- 依赖：`reqwest (default-features=false, features=["json"])`、`serde`/`serde_json`、`tokio`、`uuid (v4)`。loopback 纯 HTTP，不引 TLS。
- 该 crate 是 P1 `dsh-proxy` 的 unary 子集；P1 到来时它成为 proxy 的内部实现，不返工。

### 3.2 新增 crate：`dsh-profile`（profile 文件与 CLI 任务，零 Tauri 依赖）

三个职责，各自是纯逻辑可单测：

1. **home/profile 定位**：`resolve_dsh_home()`（DSH_HOME env 规则同上游）、`profile_dir()` → `<home>/profiles/web`、`patch_file()` → `<dir>/cordis.patch.yml`。
2. **patch 托管行读写**：`set_disabled(path, entry_id, disabled)`：
   - 语义：**显式意图**——删除该 entryId 的全部我方行后，追加恰好一条：
     ```yaml
     - id: <entryId>  # dsh-client
       disabled: <true|false>  # dsh-client
     ```
   - 文本级操作：只识别带 `# dsh-client` 行尾标记的行，绝不解析/改动其他行（`!!js` 安全）。
   - 幂等：同一意图重复执行结果一致；enable 也写 `disabled: false`（显式覆盖低层 disable，避免"删行后仍 disabled"的二段时序）。
   - 原子写：同目录临时文件 + rename（对齐上游 include 的写策略）。
3. **CLI 任务执行器** `PluginTaskRunner`：
   - 复用 `dsh-supervisor::resolve_launch` 的解析结果（同一个 dsh 二进制），把参数换成 `plugin --profile web <args...>`（需要 supervisor 侧把"解析"与"拼 daemon 参数"拆开暴露，见 §7）。
   - 串行队列（pnpm 对 profile 目录有锁），piped 输出流式读出，尾部环形缓冲（cap 500 行，与 supervisor LOG_TAIL_CAP 一致）。
   - 任务状态：`{ taskId, kind: 'install'|'remove', spec, status: 'running'|'done'|'failed', outputTail, exitCode? }`。
   - 依赖：允许依赖 `dsh-supervisor`（单向，拿 resolve）；不依赖 Tauri。

### 3.3 syscall 表（Tauri commands，保持小表）

| 命令 | 参数 → 返回 | 说明 |
|---|---|---|
| `dsh_api_call` | `{ method, payload }` → `serde_json::Value` | 通用 RPC 透传；daemon 未 ready → 结构化错误字符串 |
| `plugin_set_enabled` | `{ entryId, disabled }` → `()` | 写 patch 托管行（热生效由前端轮询验证） |
| `plugin_install` | `{ spec }` → `{ taskId }` | 入队 `dsh plugin --profile web add <spec>` |
| `plugin_remove` | `{ spec }` → `{ taskId }` | 入队 `... remove <spec>` |
| `open_plugins_window` | `()` → `()` | show/focus 插件窗口（splash 入口按钮） |

事件：`plugins://task`（任务状态变更推送，payload 即 §3.2-3 的任务状态）。DTO 全部 `camelCase`，与项目惯例一致。

### 3.4 前端

- **不引 router**：`src/main.ts` 按 `getCurrentWindow().label` 挂载 `App.vue`（main）或 `plugins/PluginsApp.vue`（plugins）。
- `tauri.conf.json` 静态声明第二窗口：`{ label: "plugins", title: "插件管理", width: 1024, height: 680, visible: false }`；主窗口导航逻辑必须保持只作用于 main label（实现时核对 lib.rs 的窗口选取）。
- 新增 `src/plugins/`：`PluginsApp.vue`（布局：左清单/右详情 + 顶部 daemon 状态条）、`plugins.ts`（TS 类型镜像 + invoke 封装 + 轮询 composable）、按需拆 `InventoryList.vue` / `DynamicList.vue` / `SettingsPanel.vue` / `InstallDialog.vue`。
- **轮询**：窗口可见（`document.visibilityState`）时每 3s 调 `dsh_api_call('pluginInventory/list', {})` 与 `dsh_api_call('dynamicCordisRunner/inventory', {})`（Remote 的 `args` 壳由 bridge 自动包装）；失败（daemon 不 ready）显示等待态。不假装有推送；P1 WS 落地后再迁移。
- 不引 Pinia（YAGNI）；样式沿用现有 CSS 变量与暗色主题；中文文案。

### 3.5 页面行为细节

**清单区（loader 插件）**：entryId、moduleName、enabled 开关、fiberPhase 徽标（failed 红色高亮）。开关切换 → `plugin_set_enabled` → 本地置"待生效"态 → 轮询确认 `enabled` 与意图一致（10s 超时 → 提示"HMR 未生效，可重启 daemon"）。

**设置区**：`settings.describe` 拉取 namespace 列表（进入该区时拉取，写后重拉）。每个 ns：脱敏值只读视图 + schema 折叠查看 + **JSON 文本编辑 user 层**（`settings.update`，带 `expectedRevision`；`settings-conflict` → 重拉并提示"已被他处修改"）；secret 槽位逐行列出（`path` + 已配置/未配置 + 写-only 输入 + 清除按钮，清除走 `settings.mutate` unset）；`applies: 'restart'` 的 ns 保存后提示重启生效。**有意不做 schema 驱动表单**（第一版 JSON 编辑，后续迭代）。

**安装/移除**：对话框输入 npm 包 spec，固定文案提示"安装即执行任意第三方代码，请确认来源可信"；提交后出任务卡片（转圈 + 输出尾部滚动 + 完成/失败）；`done` 后显示"需重启 daemon 生效"+ 调既有 `daemon_restart` 的按钮。移除同理（输入为包名）。

**动态插件区**：卡片列表（pluginId、name/purpose、所属 agentId、当前包、host/client 两半状态、失败诊断），操作"停止"（`stopFromPanel`）与"删除"（`undefineFromPanel`，二次确认），标注"会话级，daemon 重启即失"。

## 4. 错误处理

- 命令全部 `Result<T, String>`；`ApiError::Rpc` 的 `code`/`message` 原样进字符串（`[settings-conflict] ...`），前端对 `settings-conflict` 做专门文案。
- daemon 未 ready / 崩溃中：所有 `dsh_api_call` 快速失败，窗口顶部状态条给出引导。
- patch 写入失败（文件不存在/无权限）：报出文件路径与原因；profile 目录不存在时提示"先启动一次 daemon 以初始化 profile"。
- pnpm 缺失：CLI 退出码 127 → 任务失败，提示安装 pnpm（上游 stderr 原文转发）。

## 5. 安全

- 前端无 daemon 端口、无特权头能力；所有 API 经 Rust loopback 转发（机制层，策略留在 dsh 自身栅栏）。
- `dsh_api_call` 虽通用，但仅我方 webview 可达（Tauri command 面），等价于上游 Web UI 在浏览器里的能力；method 形状校验防 URL 注入。
- patch 文件只写托管行，备份策略：首次写入前复制一份 `cordis.patch.yml.dsh-client.bak`（仅在不存在时创建）。
- 安装对话框强制明示第三方代码执行风险。
- 密钥永不出现在响应里（上游已脱敏）；secret 输入框 write-only，不做本地留存。

## 6. 测试策略

- `dsh-bridge`：信封组包/解包纯函数单测（含 void 结果无 `value`、rpcId 回声不符、业务错误三分类）；一个 tokio 集成测试起本地 HTTP 假服务验证往返（可选，标记 ignore 默认不跑）。
- `dsh-profile`：托管行读写单测（追加/删除/幂等/保留 `!!js` 与他行/原子写/标记行归属）+ home 解析单测（env 覆盖、空串、默认）+ 任务队列串行性单测（假命令）。
- `dsh-supervisor`：仅做 resolve 拆分的重构，保持既有测试全绿，zero-dep 不破。
- 前端：`vue-tsc --noEmit` 必须过；`pnpm build` 过。
- 真机验证：`pnpm tauri dev`，手动过 M1–M5 的验收路径（含一次真实安装/启停/设置编辑）。

## 7. 对既有代码的触碰点

- `src-tauri/Cargo.toml`：workspace 加两个成员；app 加 `dsh-bridge`、`dsh-profile` 依赖。
- `dsh-supervisor/src/resolve.rs`：把"解析出 program + dsh 参数前缀"与"拼 `web --port 0`"拆开暴露（供 dsh-profile 复用），行为不变。
- `src-tauri/src/lib.rs`：注册 5 个新命令 + `plugins://task` 转发 task + 窗口打开命令；确认导航逻辑只作用 main 窗口。
- `tauri.conf.json`：第二窗口声明。`capabilities/default.json`：仍只 `core:default`（新命令自动可用，窗口操作走 core）。
- `src/main.ts`、`src/App.vue`（入口按钮）、新增 `src/plugins/*`。
- `docs/architecture.md`：完成后补一节（插件管理 = 首个 syscall 扩展实例）。

## 8. 交付切片（每片可独立验收）

1. **M1** `dsh-bridge` + `dsh_api_call` + 窗口骨架 + 只读清单（loader + 动态）——端到端打通
2. **M2** 启用/禁用（`dsh-profile` patch 写入 + 轮询验证 + 备份）
3. **M3** 安装/移除（任务队列 + `plugins://task` + 重启引导）
4. **M4** 设置编辑（describe/update/mutate + secret + revision 冲突）
5. **M5** 动态插件操作（stop/undefine）+ 打磨 + 文档补节

## 9. 已知边界（明确不做 / 后续迭代）

- schema 驱动设置表单（第一版 JSON 编辑）。
- 嵌套 entry（`a:b`）的启停可能因上游 patch 定址限制而失败——如实报错，不做树内手术。
- npm registry 搜索/自动补全（用户手输 spec）。
- 插件配置项级的 per-entry `config` 编辑（那是 patch 层手术，风险高；设置面已覆盖运行态配置）。
- WS 事件推送（P1 dsh-proxy 落地后迁移轮询）。
