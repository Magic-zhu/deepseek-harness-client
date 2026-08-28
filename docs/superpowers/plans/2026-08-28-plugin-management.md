# 插件管理功能实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在 Tauri 客户端里提供 dsh 插件的完整生命周期管理：清单+状态、启用/禁用、安装/移除、每插件设置编辑、动态插件查看/停止/删除，落点为独立管理窗口。

**Architecture:** 新增两个零 Tauri 依赖的 Rust crate：`dsh-bridge`（daemon `/api` wire 信封客户端，一切 RPC 经 Rust loopback 转发，webview 永远不直连）与 `dsh-profile`（`cordis.patch.yml` 文本级托管行 + `dsh plugin` CLI 串行任务队列）。Tauri 层只加 5 个小命令 + 1 个事件通道。前端不引 router，按窗口 label 挂载不同根组件，新增 `src/plugins/`。

**Tech Stack:** Rust（tokio/reqwest/serde/uuid）、Tauri 2、Vue 3 `<script setup>` + TS、pnpm + Vite 6。

**Spec:** `docs/superpowers/specs/2026-08-27-plugin-management-design.md` —— 本计划逐项对齐 spec §3 架构、§4 错误处理、§5 安全、§6 测试策略、§7 触碰点、§8 交付切片。读任务前先读 spec。

## Global Constraints

每个任务都隐含以下约束（逐字来自 spec 与本仓库既有惯例）：

- UI 文案一律中文；DTO 全部 `#[serde(rename_all = "camelCase")]`；Tauri 命令返回 `Result<T, String>`。
- 事件名 URI 命名空间式：`plugins://task`。
- **绝不 YAML 解析/求值 `cordis.patch.yml`**（含 `!!js` 方言）；只做带 `# dsh-client` 行尾标记的文本级托管行。
- 前端不引 router、不引 Pinia；前端永远不直连 daemon HTTP（一切过 `dsh_api_call` 命令）。
- Vite 锁定 6.x（本机 Node 20.4 跑不动 Vite 7）；不升级任何既有依赖版本。
- Windows 子进程一律 `cmd /c` 包裹 + `CREATE_NO_WINDOW`（复用 supervisor 的 spawn 路径，不新写第三份）；pnpm 调用必须带 `CI=true`。
- 本机手动验证命令（先杀掉残留 vite，5173 被占会让 `pnpm tauri dev` 起错前端）：
  ```bash
  DSH_CLIENT_BIN="C:\nvm\v24.15.0\node.exe C:\nvm\v24.15.0\node_modules\@deepseek-ai\dsh\lib\bin.js" pnpm tauri dev
  ```
- Cargo 命令都在 `src-tauri/` 下执行（workspace 根在 `src-tauri/Cargo.toml`）。

## 文件结构

**新建：**

| 文件 | 职责 |
|---|---|
| `src-tauri/crates/dsh-bridge/Cargo.toml` `src/lib.rs` | wire 信封：组包/解包纯函数 + `ApiClient`（reqwest loopback） |
| `src-tauri/crates/dsh-profile/Cargo.toml` `src/lib.rs` | crate 门面，re-export |
| `src-tauri/crates/dsh-profile/src/home.rs` | `$DSH_HOME` 解析、profile/patch 路径拼接（纯函数） |
| `src-tauri/crates/dsh-profile/src/patch.rs` | 托管行读写：删除+追加、原子写、首写备份 |
| `src-tauri/crates/dsh-profile/src/tasks.rs` | `PluginTaskRunner` 串行队列 + 输出环形缓冲 |
| `src/plugins/plugins.ts` | TS 类型镜像 + invoke 封装 + 轮询 composable |
| `src/plugins/PluginsApp.vue` | 插件窗口外壳：状态条 + 页签 + 通知条 |
| `src/plugins/InventoryList.vue` | loader 插件清单（M2 加开关） |
| `src/plugins/DynamicList.vue` | 动态插件卡片（M5 加停止/删除） |
| `src/plugins/InstallDialog.vue` | 安装/移除对话框（mode 复用） |
| `src/plugins/TaskCenter.vue` | 任务卡片 + 重启引导 |
| `src/plugins/SettingsPanel.vue` | 设置命名空间查看/编辑 |

**修改：**

| 文件 | 改动 |
|---|---|
| `src-tauri/Cargo.toml` | workspace 加 2 个成员；app 加 `dsh-bridge`、`dsh-profile` 依赖 |
| `src-tauri/crates/dsh-supervisor/src/resolve.rs` | 拆出 `DshInvocation`（program+前缀）与 `LaunchPlan::spawn_env`，行为不变 |
| `src-tauri/crates/dsh-supervisor/src/lib.rs` | re-export 新符号 |
| `src-tauri/src/lib.rs` | 5 个新命令 + `plugins://task` 转发 + 状态管理 |
| `src-tauri/tauri.conf.json` | 第二窗口 `plugins`（visible:false） |
| `src-tauri/capabilities/default.json` | `windows` 加 `"plugins"`（权限集不变，仍 `core:default`） |
| `src/main.ts` | 按窗口 label 选根组件 |
| `src/App.vue` | splash 加"插件管理"入口按钮 |
| `src/styles.css` | 追加插件窗口样式（全部 `.plugins` 作用域内，不影响 splash） |
| `docs/architecture.md` | M5 补一节 |

---

### Task 1: `dsh-bridge` crate —— 信封纯函数 + ApiClient

**Files:**
- Create: `src-tauri/crates/dsh-bridge/Cargo.toml`
- Create: `src-tauri/crates/dsh-bridge/src/lib.rs`
- Modify: `src-tauri/Cargo.toml`

**Interfaces:**
- Consumes: 无（第一个 crate）。
- Produces（后续任务依赖这些确切签名）:
  - `dsh_bridge::ApiClient::new(port: u16) -> ApiClient`
  - `dsh_bridge::ApiClient::call(&self, method: &str, payload: serde_json::Value) -> Result<serde_json::Value, ApiError>`（void 结果返回 `Value::Null`）
  - `dsh_bridge::ApiError`（`Display` 输出：`Transport` → `传输失败：…`；`Rpc` → `[code] message`；`Protocol` → `协议错误：…`）
  - 纯函数 `validate_method` / `wrap_payload` / `build_body` / `parse_response`（单测直接打这些）

- [ ] **Step 1: 建 crate 骨架并接入 workspace**

`src-tauri/Cargo.toml` 两处修改：

```toml
[workspace]
members = ["crates/dsh-supervisor", "crates/dsh-bridge"]
```

```toml
[dependencies]
dsh-bridge = { path = "crates/dsh-bridge" }
dsh-supervisor = { path = "crates/dsh-supervisor" }
```

`src-tauri/crates/dsh-bridge/Cargo.toml`：

```toml
[package]
name = "dsh-bridge"
version = "0.1.0"
edition = "2021"
description = "Wire client for the dsh daemon /api envelope: request build, response validation, error folding. Loopback plain HTTP only."

[dependencies]
reqwest = { version = "0.12", default-features = false, features = ["json"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
uuid = { version = "1", features = ["v4"] }

[dev-dependencies]
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

注意：`reqwest` 不开 TLS feature——loopback 纯 HTTP，不引 TLS 栈（spec §3.1）。

- [ ] **Step 2: 写失败的单元测试**

`src-tauri/crates/dsh-bridge/src/lib.rs` 先只放测试（实现下一步行补）：

```rust
//! dsh-bridge — the dsh daemon /api wire envelope, client side.

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn validate_method_accepts_static_and_remote() {
        assert!(validate_method("settings.describe").is_ok());
        assert!(validate_method("pluginInventory/list").is_ok());
        assert!(validate_method("dynamicCordisRunner/stopFromPanel").is_ok());
    }

    #[test]
    fn validate_method_rejects_path_smuggling() {
        for bad in ["", "foo", "a//b", "a/b/c", "a.b.c", "../etc", "a b/c", "a?b/c", "/list", "x/"] {
            assert!(validate_method(bad).is_err(), "应拒绝 {bad:?}");
        }
    }

    #[test]
    fn wrap_payload_only_wraps_remote_methods() {
        assert_eq!(wrap_payload("pluginInventory/list", json!({})), json!({ "args": {} }));
        assert_eq!(
            wrap_payload("dynamicCordisRunner/stopFromPanel", json!({ "pluginId": "p" })),
            json!({ "args": { "pluginId": "p" } }),
        );
        let plain = json!({ "ns": "x" });
        assert_eq!(wrap_payload("settings.update", plain.clone()), plain);
    }

    #[test]
    fn build_body_mints_rpc_id_and_envelope() {
        let (rpc_id, body) = build_body("settings.describe", json!({}));
        assert!(!rpc_id.is_empty());
        assert_eq!(body["type"], "client-request");
        assert_eq!(body["rpcId"], rpc_id);
        assert_eq!(body["method"], "settings.describe");
        assert_eq!(body["payload"], json!({}));
    }

    #[test]
    fn parse_response_ok_with_value() {
        let body = json!({
            "type": "server-response", "rpcId": "r1",
            "result": { "ok": true, "value": { "entries": [] } },
        });
        assert_eq!(parse_response(&body, "r1").unwrap(), json!({ "entries": [] }));
    }

    #[test]
    fn parse_response_ok_void_has_no_value() {
        // Typert void 结果的响应没有 value 字段。
        let body = json!({
            "type": "server-response", "rpcId": "r1",
            "result": { "ok": true },
        });
        assert_eq!(parse_response(&body, "r1").unwrap(), serde_json::Value::Null);
    }

    #[test]
    fn parse_response_business_error_keeps_code_message_details() {
        let body = json!({
            "type": "server-response", "rpcId": "r1",
            "result": { "ok": false, "error": {
                "code": "settings-conflict", "message": "revision 过期",
                "details": { "ns": "a", "expected": 1, "actual": 2 },
            } },
        });
        match parse_response(&body, "r1") {
            Err(ApiError::Rpc { code, message, details }) => {
                assert_eq!(code, "settings-conflict");
                assert_eq!(message, "revision 过期");
                assert_eq!(details["actual"], 2);
            }
            other => panic!("应为 Rpc 错误，得到 {other:?}"),
        }
    }

    #[test]
    fn parse_response_rejects_rpc_id_mismatch_and_bad_type() {
        let body = json!({ "type": "server-response", "rpcId": "r2", "result": { "ok": true } });
        assert!(matches!(parse_response(&body, "r1"), Err(ApiError::Protocol(_))));
        let not_response = json!({ "type": "server-request", "rpcId": "r1" });
        assert!(matches!(parse_response(&not_response, "r1"), Err(ApiError::Protocol(_))));
    }

    #[test]
    fn api_error_display_formats() {
        let rpc = ApiError::Rpc { code: "settings-conflict".into(), message: "过期".into(), details: json!({}) };
        assert_eq!(rpc.to_string(), "[settings-conflict] 过期");
        assert_eq!(ApiError::Transport("超时".into()).to_string(), "传输失败：超时");
    }
}
```

- [ ] **Step 3: 跑测试确认失败**

```bash
cd src-tauri && cargo test -p dsh-bridge
```

预期：编译失败（`validate_method` 等未定义）。

- [ ] **Step 4: 实现 lib.rs（放在测试模块之前）**

```rust
//! dsh-bridge — the dsh daemon /api wire envelope, client side.
//!
//! One job: build a `client-request`, POST it, validate the
//! `server-response`, and fold business errors into [`ApiError::Rpc`].
//! Loopback plain HTTP only; the daemon's own trust fence pins the rest.

use std::time::Duration;

use serde_json::{json, Value};

/// Unary timeout, mirroring upstream `AbstractApiClient`.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub enum ApiError {
    /// Connect failure, timeout, or a non-2xx status (transport only; HTTP
    /// status never carries business meaning in this envelope).
    Transport(String),
    /// Business error from the daemon: code/message/details verbatim.
    Rpc { code: String, message: String, details: Value },
    /// Malformed envelope, bad method shape, or rpcId echo mismatch.
    Protocol(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::Transport(msg) => write!(f, "传输失败：{msg}"),
            ApiError::Rpc { code, message, .. } => write!(f, "[{code}] {message}"),
            ApiError::Protocol(msg) => write!(f, "协议错误：{msg}"),
        }
    }
}

impl std::error::Error for ApiError {}

/// One method-name segment: ASCII alnum plus `-`/`_`, nonempty.
fn valid_segment(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Method shape whitelist: `<domain>.<name>` (static method) or
/// `<ns>/<name>` (Typert remote). Rejects path characters so a webview can
/// never smuggle a URL into the POST path.
pub fn validate_method(method: &str) -> Result<(), ApiError> {
    let bad = || ApiError::Protocol(format!("非法 method 形状：{method:?}"));
    if let Some((ns, name)) = method.split_once('/') {
        return if valid_segment(ns) && valid_segment(name) { Ok(()) } else { Err(bad()) };
    }
    if let Some((domain, name)) = method.split_once('.') {
        return if valid_segment(domain) && valid_segment(name) { Ok(()) } else { Err(bad()) };
    }
    Err(bad())
}

/// Typert remotes require the payload wrapped as exactly `{ "args": {...} }`;
/// static methods take the payload verbatim. The frontend never writes the
/// `args` shell itself.
pub fn wrap_payload(method: &str, payload: Value) -> Value {
    if method.contains('/') { json!({ "args": payload }) } else { payload }
}

/// Build the wire body; the minted rpcId is returned for echo validation.
pub fn build_body(method: &str, payload: Value) -> (String, Value) {
    let rpc_id = uuid::Uuid::new_v4().to_string();
    let body = json!({
        "type": "client-request",
        "rpcId": rpc_id,
        "method": method,
        "payload": wrap_payload(method, payload),
    });
    (rpc_id, body)
}

/// Validate a raw response body: envelope shape, rpcId echo, result fold.
/// `void` results have no `value` field and surface as `Value::Null`.
pub fn parse_response(body: &Value, expected_rpc_id: &str) -> Result<Value, ApiError> {
    if body.get("type").and_then(Value::as_str) != Some("server-response") {
        return Err(ApiError::Protocol(format!("响应 type 非 server-response：{body}")));
    }
    let echoed = body.get("rpcId").and_then(Value::as_str).unwrap_or_default();
    if echoed != expected_rpc_id {
        return Err(ApiError::Protocol(format!("rpcId 回声不符：期望 {expected_rpc_id}，收到 {echoed:?}")));
    }
    let result = body.get("result").ok_or_else(|| ApiError::Protocol("响应缺 result 字段".into()))?;
    match result.get("ok").and_then(Value::as_bool) {
        Some(true) => Ok(result.get("value").cloned().unwrap_or(Value::Null)),
        Some(false) => {
            let error = result.get("error").cloned().unwrap_or(Value::Null);
            let code = error.get("code").and_then(Value::as_str).unwrap_or("internal").to_string();
            let message = error.get("message").and_then(Value::as_str).unwrap_or("（无 message）").to_string();
            let details = error.get("details").cloned().unwrap_or(Value::Null);
            Err(ApiError::Rpc { code, message, details })
        }
        _ => Err(ApiError::Protocol(format!("result.ok 缺失或非布尔：{result}"))),
    }
}

/// Loopback unary client. One client per call site is fine at management-UI
/// call rates; the port is re-read per call because a restarted daemon gets
/// a fresh one.
pub struct ApiClient {
    http: reqwest::Client,
    base: String,
}

impl ApiClient {
    pub fn new(port: u16) -> Self {
        let http = reqwest::Client::builder()
            .timeout(CALL_TIMEOUT)
            .build()
            .expect("reqwest client build");
        Self { http, base: format!("http://127.0.0.1:{port}") }
    }

    pub async fn call(&self, method: &str, payload: Value) -> Result<Value, ApiError> {
        validate_method(method)?;
        let (rpc_id, body) = build_body(method, payload);
        let url = format!("{}/api/{method}", self.base);
        let response = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| ApiError::Transport(e.to_string()))?;
        if !response.status().is_success() {
            return Err(ApiError::Transport(format!("HTTP {}", response.status())));
        }
        let raw: Value = response.json().await.map_err(|e| ApiError::Protocol(e.to_string()))?;
        parse_response(&raw, &rpc_id)
    }
}
```

- [ ] **Step 5: 跑测试确认通过**

```bash
cd src-tauri && cargo test -p dsh-bridge
```

预期：9 个测试全 PASS。

- [ ] **Step 6: 补一个本地假服务往返集成测试（标记 ignore）**

在测试模块末尾追加：

```rust
    /// Loopback stub server: one request in, fixed envelope out (rpcId echoed
    /// from the request). Run on demand with `-- --ignored`.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "本地假服务集成测试，按需跑：cargo test -p dsh-bridge -- --ignored"]
    async fn round_trip_against_stub_server() {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = vec![0u8; 65536];
            let n = stream.read(&mut buf).unwrap();
            let request = String::from_utf8_lossy(&buf[..n]).to_string();
            let body_text = request.split("\r\n\r\n").nth(1).unwrap_or("{}").to_string();
            let rpc_id = serde_json::from_str::<serde_json::Value>(&body_text)
                .ok()
                .and_then(|v| v["rpcId"].as_str().map(str::to_owned))
                .unwrap_or_default();
            let response_body = serde_json::json!({
                "type": "server-response", "rpcId": rpc_id,
                "result": { "ok": true, "value": { "pong": true } },
            })
            .to_string();
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                response_body.len()
            );
            stream.write_all(head.as_bytes()).unwrap();
            stream.write_all(response_body.as_bytes()).unwrap();
            tx.send(request).unwrap();
        });

        let client = ApiClient::new(port);
        let value = client.call("pluginInventory/list", json!({})).await.unwrap();
        assert_eq!(value, json!({ "pong": true }));

        let request = rx.recv().unwrap();
        assert!(request.starts_with("POST /api/pluginInventory/list HTTP/1.1"), "{request}");
        assert!(request.contains(r#""payload":{"args":{}}"#), "Remote 方法应自动包 args 壳：{request}");
    }
```

跑 `cd src-tauri && cargo test -p dsh-bridge -- --ignored` 确认 PASS；默认 `cargo test` 确认它被跳过。

- [ ] **Step 7: Commit**

```bash
git add src-tauri/Cargo.toml src-tauri/crates/dsh-bridge
git commit -m "feat(bridge): dsh-bridge crate —— /api 信封组包/解包 + loopback ApiClient"
```

---

### Task 2: `dsh_api_call` 命令（syscall 表第一条）

**Files:**
- Modify: `src-tauri/src/lib.rs`

**Interfaces:**
- Consumes: Task 1 的 `dsh_bridge::ApiClient` / `ApiError`；既有 `DaemonState { supervisor }`（`supervisor.status().await.port: Option<u16>`）。
- Produces: Tauri 命令 `dsh_api_call({ method, payload }) -> serde_json::Value`，前端 Task 4 的 `apiCall()` 依赖它。错误字符串约定：`daemon 未启动…` / `daemon 未就绪…` / `ApiError` 的 Display 原文。

- [ ] **Step 1: 加命令**

`src-tauri/src/lib.rs` 在 `preflight_check` 之后追加：

```rust
/// Generic daemon /api pass-through. The webview can never satisfy the
/// daemon's trust fence (Origin/Sec-Fetch-Site), so every API call crosses
/// this bridge. `Err` strings: `ApiError` Display verbatim (`[code] message`
/// for business errors — the settings UI pattern-matches the prefix).
#[tauri::command]
async fn dsh_api_call(
    state: Option<tauri::State<'_, DaemonState>>,
    method: String,
    payload: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let Some(state) = state else {
        return Err("daemon 未启动（preflight 未通过），请先解决运行环境问题".to_string());
    };
    let port = state
        .supervisor
        .status()
        .await
        .port
        .ok_or_else(|| "daemon 未就绪：尚无可用端口（启动中或已崩溃）".to_string())?;
    dsh_bridge::ApiClient::new(port)
        .call(&method, payload)
        .await
        .map_err(|err| err.to_string())
}
```

注册进 `invoke_handler`：

```rust
        .invoke_handler(tauri::generate_handler![
            daemon_status,
            daemon_log_tail,
            daemon_restart,
            daemon_stop,
            preflight_check,
            dsh_api_call
        ])
```

说明：preflight 失败时 `DaemonState` 未 manage，参数用 `Option<State>` 拿到可读的快速失败而不是 Tauri 的 "state not managed"。

- [ ] **Step 2: 编译 + 全量测试**

```bash
cd src-tauri && cargo check && cargo test
```

预期：编译过；既有测试全绿。

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/lib.rs
git commit -m "feat: dsh_api_call 命令 —— daemon /api 经 Rust loopback 透传"
```

---

### Task 3: 插件窗口骨架（第二窗口 + 入口按钮 + 外壳）

**Files:**
- Modify: `src-tauri/tauri.conf.json`
- Modify: `src-tauri/capabilities/default.json`
- Modify: `src-tauri/src/lib.rs`（加 `open_plugins_window`）
- Modify: `src/main.ts`
- Modify: `src/App.vue`
- Modify: `src/styles.css`
- Create: `src/plugins/PluginsApp.vue`

**Interfaces:**
- Consumes: 无。
- Produces: 窗口 label `plugins`；命令 `open_plugins_window()`；`PluginsApp.vue` 外壳（后续任务往里填 4 个页签面板，占位结构见本任务模板里的注释位置）；`main.ts` 的 label 分支惯例。

- [ ] **Step 1: tauri.conf.json 声明第二窗口**

`app.windows` 数组追加（主窗口对象不动）：

```json
      {
        "label": "plugins",
        "title": "插件管理",
        "width": 1024,
        "height": 680,
        "minWidth": 800,
        "minHeight": 520,
        "visible": false
      }
```

- [ ] **Step 2: capabilities 扩展到 plugins 窗口**

`src-tauri/capabilities/default.json` 改为：

```json
{
  "$schema": "../gen/schemas/desktop-schema.json",
  "identifier": "default",
  "description": "主窗口与插件窗口的基础能力：事件与窗口 API，仅此而已。",
  "windows": ["main", "plugins"],
  "permissions": ["core:default"]
}
```

（权限集不变；不加 `"plugins"` 则插件窗口的 `listen` 会被拒。）

- [ ] **Step 3: `open_plugins_window` 命令**

`src-tauri/src/lib.rs` 追加并注册：

```rust
/// Show and focus the plugin management window (declared hidden at startup
/// in tauri.conf.json, so it is already loaded and subscribed).
#[tauri::command]
fn open_plugins_window(app: tauri::AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("plugins")
        .ok_or_else(|| "插件窗口未创建（tauri.conf.json 缺少 plugins 声明）".to_string())?;
    window.show().map_err(|e| e.to_string())?;
    window.set_focus().map_err(|e| e.to_string())?;
    Ok(())
}
```

同时核对：`forward_task` 里的导航逻辑已经显式 `get_webview_window("main")`，插件窗口不会被导航波及——无需改动，确认即可。

- [ ] **Step 4: main.ts 按 label 选根组件**

整个 `src/main.ts` 替换为：

```ts
import { createApp } from 'vue'
import { getCurrentWindow } from '@tauri-apps/api/window'
import App from './App.vue'
import PluginsApp from './plugins/PluginsApp.vue'
import './styles.css'

const root = getCurrentWindow().label === 'plugins' ? PluginsApp : App
createApp(root).mount('#app')
```

- [ ] **Step 5: PluginsApp.vue 外壳**

创建 `src/plugins/PluginsApp.vue`：

```vue
<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { listen } from '@tauri-apps/api/event'
import type { UnlistenFn } from '@tauri-apps/api/event'
import { DAEMON_CHANNELS, fetchStatus } from '../daemon'
import type { DaemonEvent, DaemonStatus } from '../daemon'

/** 顶部 daemon 状态条：本窗口的一切数据都依赖 daemon 就绪。 */
const status = ref<DaemonStatus | null>(null)
let unlisteners: UnlistenFn[] = []

const daemonReady = computed<boolean>(
  () => status.value?.state === 'running' && status.value.port !== null,
)

const statusText = computed<string>(() => {
  const s = status.value
  if (!s) return '正在获取 daemon 状态…'
  switch (s.state) {
    case 'starting':
      return 'daemon 启动中…'
    case 'running':
      return daemonReady.value ? `daemon 运行中（端口 ${s.port}）` : 'daemon 运行中（等待端口）'
    case 'backoff':
      return 'daemon 崩溃重试中'
    case 'stopped':
      return 'daemon 已停止'
  }
})

async function refreshStatus(): Promise<void> {
  const next = await fetchStatus().catch(() => null)
  if (next) status.value = next
}

onMounted(async () => {
  const subs = await Promise.all(
    DAEMON_CHANNELS.map((name) => listen<DaemonEvent>(name, () => void refreshStatus())),
  )
  unlisteners.push(...subs)
  await refreshStatus()
})

onUnmounted(() => {
  for (const unlisten of unlisteners) unlisten()
})

type Tab = 'inventory' | 'dynamic' | 'settings' | 'tasks'
const tab = ref<Tab>('inventory')
const tabs: Array<[Tab, string]> = [
  ['inventory', '插件清单'],
  ['dynamic', '动态插件'],
  ['settings', '设置'],
  ['tasks', '任务'],
]
</script>

<template>
  <main class="plugins">
    <header class="bar">
      <h1>插件管理</h1>
      <span class="daemon-state" :data-ready="daemonReady">{{ statusText }}</span>
    </header>
    <nav class="tabs">
      <button
        v-for="[key, label] in tabs"
        :key="key"
        :class="['tab', { active: tab === key }]"
        @click="tab = key"
      >
        {{ label }}
      </button>
    </nav>
    <section class="panel">
      <p v-if="!daemonReady" class="waiting">等待 daemon 就绪后可加载数据。</p>
      <p v-else class="waiting">（后续切片交付此面板）</p>
    </section>
  </main>
</template>
```

- [ ] **Step 6: splash 入口按钮**

`src/App.vue` script 顶部 import 区加：

```ts
import { invoke } from '@tauri-apps/api/core'
```

`<script setup>` 内加：

```ts
async function openPlugins(): Promise<void> {
  await invoke('open_plugins_window').catch(() => {})
}
```

模板中 `<button v-if="showRetry" class="retry" …>` 之后加：

```html
      <button class="plugins-entry" @click="openPlugins">插件管理</button>
```

- [ ] **Step 7: styles.css 追加插件窗口基础样式**

`src/styles.css` 末尾追加：

```css
/* ---- 插件管理窗口（.plugins 作用域内，不影响 splash） ---- */

.plugins {
  min-height: 100vh;
  padding: 20px 24px 40px;
  max-width: 980px;
  margin: 0 auto;
}

.plugins .bar {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 12px;
}

.plugins .bar h1 {
  font-size: 20px;
  margin: 0;
}

.bar-actions {
  display: flex;
  gap: 8px;
}

.daemon-state {
  font-size: 12px;
  color: var(--fg-dim);
}

.daemon-state[data-ready='true'] {
  color: #6fd38c;
}

.tabs {
  display: flex;
  gap: 4px;
  margin: 16px 0 12px;
  border-bottom: 1px solid var(--border);
}

.tab {
  background: none;
  border: none;
  color: var(--fg-dim);
  padding: 8px 14px;
  cursor: pointer;
  font-size: 14px;
  border-bottom: 2px solid transparent;
}

.tab.active {
  color: var(--fg);
  border-bottom-color: var(--accent);
}

.notice {
  margin: 8px 0;
  padding: 8px 12px;
  border: 1px solid var(--accent);
  border-radius: 8px;
  font-size: 13px;
}

.waiting {
  color: var(--fg-dim);
}

.error {
  color: var(--danger);
  font-size: 13px;
}

.hint {
  color: var(--fg-dim);
  font-size: 12px;
}

.mono {
  font-family: 'Cascadia Code', Consolas, monospace;
  font-size: 12px;
}

.dim {
  color: var(--fg-dim);
}

.badge {
  padding: 2px 8px;
  border-radius: 999px;
  font-size: 12px;
  border: 1px solid var(--border);
}

.badge.on {
  color: #6fd38c;
  border-color: #2c5f3d;
}

.badge.off {
  color: var(--fg-dim);
}

.phase {
  font-size: 12px;
  color: var(--fg-dim);
}

.phase[data-phase='failed'] {
  color: var(--danger);
}

.phase[data-phase='active'] {
  color: #6fd38c;
}

.mini {
  background: rgba(77, 107, 254, 0.12);
  color: var(--fg);
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 4px 12px;
  font-size: 12px;
  cursor: pointer;
}

.mini:disabled {
  opacity: 0.5;
  cursor: default;
}

.mini.primary {
  background: var(--accent);
  border-color: var(--accent);
}

.mini.danger {
  border-color: var(--danger);
  color: var(--danger);
  background: none;
}

.plugins-entry {
  margin-top: 16px;
  background: none;
  border: 1px solid var(--border);
  color: var(--fg-dim);
  border-radius: 10px;
  padding: 8px 18px;
  font-size: 13px;
  cursor: pointer;
}

.plugins-entry:hover {
  color: var(--fg);
  border-color: var(--accent);
}
```

- [ ] **Step 8: 验证**

```bash
pnpm build
```

预期：`vue-tsc --noEmit` + `vite build` 通过。

手动（可选，M1 末尾统一验）：`pnpm tauri dev` 起应用，点"插件管理"按钮，第二窗口出现，标题"插件管理"，状态条显示 daemon 状态。

- [ ] **Step 9: Commit**

```bash
git add src-tauri/tauri.conf.json src-tauri/capabilities/default.json src-tauri/src/lib.rs src/main.ts src/App.vue src/styles.css src/plugins/PluginsApp.vue
git commit -m "feat: 插件管理窗口骨架 —— 第二窗口 + 入口按钮 + 状态条外壳"
```

---

### Task 4: 只读清单（loader + 动态）—— M1 完成

**Files:**
- Create: `src/plugins/plugins.ts`
- Create: `src/plugins/InventoryList.vue`
- Create: `src/plugins/DynamicList.vue`
- Modify: `src/plugins/PluginsApp.vue`

**Interfaces:**
- Consumes: Task 2 的 `dsh_api_call`；Task 3 的 PluginsApp 外壳。
- Produces（后续任务依赖）:
  - `apiCall<T>(method: string, payload: unknown): Promise<T>`
  - `PluginEntry` / `PluginInventorySnapshot` / `fetchInventory()`
  - `DynamicPluginRow` / `fetchDynamicInventory()`（Task 14 复用 row 类型）
  - `usePolling(loader, intervalMs?)`
  - PluginsApp 内的 `loadInventories()`、`entries`、`dynamicRows`（Task 7/11/14 在其上叠加，不重写结构）

- [ ] **Step 1: 写 plugins.ts（类型镜像 + invoke 封装 + 轮询）**

```ts
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
```

- [ ] **Step 2: 写 InventoryList.vue（只读版，开关在 Task 7 加）**

```vue
<script setup lang="ts">
import type { PluginEntry } from './plugins'

defineProps<{ entries: PluginEntry[] }>()

function phaseLabel(entry: PluginEntry): string {
  switch (entry.fiberPhase) {
    case 'pending':
      return '等待加载'
    case 'loading':
      return '加载中'
    case 'active':
      return '运行中'
    case 'failed':
      return '加载失败'
    case 'unloading':
      return '卸载中'
    case null:
      return '未加载'
  }
}
</script>

<template>
  <div class="inventory">
    <p v-if="!entries.length" class="waiting">清单为空。</p>
    <table v-else class="table">
      <thead>
        <tr>
          <th>插件</th>
          <th>模块</th>
          <th>状态</th>
          <th>启用</th>
        </tr>
      </thead>
      <tbody>
        <tr v-for="entry in entries" :key="entry.entryId">
          <td class="mono">{{ entry.entryId }}</td>
          <td class="mono dim">{{ entry.moduleName }}</td>
          <td><span class="phase" :data-phase="entry.fiberPhase ?? 'none'">{{ phaseLabel(entry) }}</span></td>
          <td><span :class="['badge', entry.enabled ? 'on' : 'off']">{{ entry.enabled ? '启用' : '禁用' }}</span></td>
        </tr>
      </tbody>
    </table>
  </div>
</template>
```

- [ ] **Step 3: 写 DynamicList.vue（只读版，操作在 Task 14 加）**

```vue
<script setup lang="ts">
import type { DynamicPluginRow } from './plugins'

defineProps<{ rows: DynamicPluginRow[] }>()

/** 当前生效包（运行中的包优先，其次 last-success，再其次最新版本）。 */
function currentPackage(row: DynamicPluginRow) {
  const id = row.activeRun?.packageId ?? row.currentPackageId
  return row.packages.find((p) => p.packageId === id) ?? row.packages[row.packages.length - 1]
}

function halfLabel(status: string | undefined): string {
  switch (status) {
    case 'running':
      return '运行中'
    case 'waiting':
      return '等待依赖'
    case 'pending':
      return '加载中'
    case 'stopped':
      return '已停止'
    case 'failed':
      return '失败'
    case 'absent':
      return '无此半'
    default:
      return '—'
  }
}

function runStatusLabel(row: DynamicPluginRow): string {
  if (row.activeRun) return '运行中'
  switch (row.latestRun?.status) {
    case 'awaiting-approval':
      return '等待批准'
    case 'starting-host':
      return '启动中'
    case 'client-pending':
      return '等待页面'
    case 'waiting':
      return '等待依赖'
    case 'rejected':
      return '已拒绝'
    case 'failed':
      return '失败'
    case 'cancelled':
      return '已取消'
    case 'stopped':
      return '已停止'
    default:
      return '未运行'
  }
}
</script>

<template>
  <div class="dynamic">
    <p class="hint">动态插件由模型在会话中定义，会话级、进程内，daemon 重启即失。</p>
    <p v-if="!rows.length" class="waiting">当前没有动态插件。</p>
    <article v-for="row in rows" :key="row.pluginId" class="dyn-card">
      <header>
        <strong>{{ currentPackage(row)?.name || row.pluginId }}</strong>
        <span class="phase" :data-phase="row.activeRun ? 'active' : 'none'">{{ runStatusLabel(row) }}</span>
      </header>
      <p v-if="currentPackage(row)?.purpose" class="dim">{{ currentPackage(row)?.purpose }}</p>
      <dl class="kv">
        <dt>pluginId</dt>
        <dd class="mono">{{ row.pluginId }}</dd>
        <dt>所属会话</dt>
        <dd class="mono">{{ row.agentId }}</dd>
        <dt>包版本数</dt>
        <dd>{{ row.packages.length }}</dd>
        <dt>host 半</dt>
        <dd>
          {{ halfLabel(row.latestRun?.host.status) }}
          <template v-if="row.latestRun?.host.waitingFor?.length">
            （等待：{{ row.latestRun.host.waitingFor.join(', ') }}）
          </template>
        </dd>
        <dt>client 半</dt>
        <dd>{{ halfLabel(row.latestRun?.client.status) }}</dd>
      </dl>
      <pre v-if="row.latestRun?.error" class="error-detail">{{ row.latestRun.error.phase }}: {{ row.latestRun.error.message }}</pre>
    </article>
  </div>
</template>
```

- [ ] **Step 4: PluginsApp.vue 接入轮询与两个面板**

`<script setup>` 顶部 import 区追加：

```ts
import InventoryList from './InventoryList.vue'
import DynamicList from './DynamicList.vue'
import { fetchDynamicInventory, fetchInventory, usePolling } from './plugins'
import type { DynamicPluginRow, PluginEntry } from './plugins'
```

`tab`/`tabs` 声明之后追加：

```ts
const entries = ref<PluginEntry[]>([])
const dynamicRows = ref<DynamicPluginRow[]>([])
const loadError = ref<string | null>(null)

async function loadInventories(): Promise<void> {
  try {
    const snap = await fetchInventory()
    entries.value = snap.entries
    loadError.value = null
  } catch (err) {
    loadError.value = String(err)
  }
  try {
    dynamicRows.value = await fetchDynamicInventory()
  } catch {
    /* 动态清单失败保留旧数据；主错误条已提示 */
  }
}

usePolling(loadInventories)
```

`<section class="panel">` 整个替换为：

```html
    <section class="panel">
      <p v-if="!daemonReady" class="waiting">等待 daemon 就绪后可加载数据。</p>
      <template v-else>
        <p v-if="loadError" class="error">{{ loadError }}</p>
        <InventoryList v-if="tab === 'inventory'" :entries="entries" />
        <DynamicList v-else-if="tab === 'dynamic'" :rows="dynamicRows" />
        <p v-else class="waiting">（后续切片交付此面板）</p>
      </template>
    </section>
```

- [ ] **Step 5: styles.css 追加清单/卡片样式**

```css
.table {
  width: 100%;
  border-collapse: collapse;
  font-size: 13px;
}

.table th,
.table td {
  text-align: left;
  padding: 6px 8px;
  border-bottom: 1px solid var(--border);
}

.table th {
  color: var(--fg-dim);
  font-weight: normal;
  font-size: 12px;
}

.dyn-card,
.ns-card,
.task-card {
  border: 1px solid var(--border);
  border-radius: 12px;
  padding: 12px 16px;
  margin-bottom: 10px;
  background: rgba(16, 26, 58, 0.5);
}

.dyn-card header,
.ns-card header,
.task-card header {
  display: flex;
  align-items: center;
  gap: 10px;
}

.kv {
  display: grid;
  grid-template-columns: 88px 1fr;
  gap: 4px 10px;
  font-size: 12px;
  margin: 8px 0;
}

.kv dt {
  color: var(--fg-dim);
}

.kv dd {
  margin: 0;
}

.error-detail {
  color: var(--danger);
}

.error-detail,
.json,
.task-out {
  background: rgba(4, 8, 20, 0.7);
  border-radius: 8px;
  padding: 8px 10px;
  font-size: 12px;
  overflow: auto;
  max-height: 220px;
  white-space: pre-wrap;
  word-break: break-all;
}
```

- [ ] **Step 6: 验证（M1 端到端）**

```bash
pnpm build
cd src-tauri && cargo check
```

然后手动验收 M1：

```bash
# 先确认没有残留 vite 占 5173（有就杀掉），再：
DSH_CLIENT_BIN="C:\nvm\v24.15.0\node.exe C:\nvm\v24.15.0\node_modules\@deepseek-ai\dsh\lib\bin.js" pnpm tauri dev
```

验收路径：splash 点"插件管理"→ 插件窗口出现 → "插件清单"页签列出 loader 插件（entryId/模块/状态徽标）→ "动态插件"页签显示空态或卡片 → 状态条变绿。daemon 杀掉时清单区显示等待态而不是报错刷屏。

- [ ] **Step 7: Commit**

```bash
git add src/plugins/ src/styles.css
git commit -m "feat: 插件窗口只读清单（loader + 动态插件），3s 轮询 —— M1 完成"
```

---

### Task 5: `dsh-profile` —— home/profile 定位

**Files:**
- Create: `src-tauri/crates/dsh-profile/Cargo.toml`
- Create: `src-tauri/crates/dsh-profile/src/lib.rs`
- Create: `src-tauri/crates/dsh-profile/src/home.rs`
- Modify: `src-tauri/Cargo.toml`（workspace members + app 依赖）

**Interfaces:**
- Consumes: 无。
- Produces:
  - `dsh_profile::home::resolve_dsh_home() -> Option<PathBuf>`
  - `dsh_profile::home::profile_dir(home: &Path) -> PathBuf`（`<home>/profiles/web`）
  - `dsh_profile::home::patch_file(profile_dir: &Path) -> PathBuf`（`<dir>/cordis.patch.yml`）
  - 常量 `DSH_HOME_ENV` / `DSH_HOME_DIR_NAME` / `PROFILE_NAME` / `PROFILE_PATCH_FILENAME`

- [ ] **Step 1: 建 crate 骨架并接入 workspace**

`src-tauri/Cargo.toml`：

```toml
[workspace]
members = ["crates/dsh-supervisor", "crates/dsh-bridge", "crates/dsh-profile"]
```

`[dependencies]` 加：

```toml
dsh-profile = { path = "crates/dsh-profile" }
```

`src-tauri/crates/dsh-profile/Cargo.toml`：

```toml
[package]
name = "dsh-profile"
version = "0.1.0"
edition = "2021"
description = "dsh profile seams: home resolution, cordis.patch.yml managed lines, dsh plugin CLI serial task runner."

[dependencies]
dsh-supervisor = { path = "../dsh-supervisor" }
serde = { version = "1", features = ["derive"] }
tokio = { version = "1", features = ["io-util", "macros", "process", "rt", "sync", "time"] }
uuid = { version = "1", features = ["v4"] }

[dev-dependencies]
tokio = { version = "1", features = ["macros", "rt-multi-thread", "time"] }
```

（`dsh-supervisor` 依赖此刻还不会被用到——Task 8/9 用；先声明避免二次改清单。）

`src-tauri/crates/dsh-profile/src/lib.rs`：

```rust
//! dsh-profile — filesystem and CLI seams of a dsh profile.
//!
//! Everything here is plain logic over paths/text/processes with zero Tauri
//! imports, so it is unit-testable end to end.

pub mod home;
```

- [ ] **Step 2: 写失败测试**

`src-tauri/crates/dsh-profile/src/home.rs` 先只放测试：

```rust
//! `$DSH_HOME` resolution, mirroring upstream `home-paths`: the env var
//! (blank = unset) wins; otherwise `~/.dsh`.

#[cfg(test)]
mod tests {
    use super::dsh_home_from;
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn getter(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> =
            pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |key| map.get(key).cloned()
    }

    #[cfg(windows)]
    const HOME_VAR: &str = "USERPROFILE";
    #[cfg(unix)]
    const HOME_VAR: &str = "HOME";

    #[test]
    fn env_override_wins() {
        let home = dsh_home_from(getter(&[("DSH_HOME", "D:/dsh-data"), (HOME_VAR, "C:/Users/x")])).unwrap();
        assert_eq!(home, PathBuf::from("D:/dsh-data"));
    }

    #[test]
    fn blank_env_falls_back_to_default() {
        for blank in ["", "   "] {
            let home = dsh_home_from(getter(&[("DSH_HOME", blank), (HOME_VAR, "/home/x")])).unwrap();
            assert_eq!(home, PathBuf::from("/home/x").join(".dsh"), "空白值 {blank:?} 应视为未设置");
        }
    }

    #[test]
    fn missing_env_uses_home_dot_dsh() {
        let home = dsh_home_from(getter(&[(HOME_VAR, "/home/x")])).unwrap();
        assert_eq!(home, PathBuf::from("/home/x").join(".dsh"));
    }

    #[test]
    fn nothing_resolvable_yields_none() {
        assert!(dsh_home_from(getter(&[])).is_none());
    }

    #[test]
    fn profile_and_patch_paths_join() {
        let home = PathBuf::from("/data/.dsh");
        let profile = super::profile_dir(&home);
        assert_eq!(profile, PathBuf::from("/data/.dsh/profiles/web"));
        assert_eq!(super::patch_file(&profile), PathBuf::from("/data/.dsh/profiles/web/cordis.patch.yml"));
    }
}
```

- [ ] **Step 3: 跑测试确认失败**

```bash
cd src-tauri && cargo test -p dsh-profile
```

预期：编译失败（`dsh_home_from` 等未定义）。

- [ ] **Step 4: 实现 home.rs（放在测试模块之前）**

```rust
//! `$DSH_HOME` resolution, mirroring upstream `home-paths`: the env var
//! (blank = unset) wins; otherwise `~/.dsh`.

use std::path::{Path, PathBuf};

pub const DSH_HOME_ENV: &str = "DSH_HOME";
pub const DSH_HOME_DIR_NAME: &str = ".dsh";
/// The only profile this client drives (`dsh web` ≡ `--profile web`).
pub const PROFILE_NAME: &str = "web";
pub const PROFILE_PATCH_FILENAME: &str = "cordis.patch.yml";

/// Pure core of [`resolve_dsh_home`]: the env getter is injected so tests
/// never touch the process environment (parallel-test safe).
pub fn dsh_home_from(get: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    if let Some(value) = get(DSH_HOME_ENV) {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }
    platform_home_from(get).map(|home| home.join(DSH_HOME_DIR_NAME))
}

#[cfg(windows)]
fn platform_home_from(get: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    get("USERPROFILE").map(PathBuf::from)
}

#[cfg(unix)]
fn platform_home_from(get: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    get("HOME").map(PathBuf::from)
}

/// Resolve against the real process environment. The daemon is our child and
/// inherits the same environment, so both sides resolve the same home.
pub fn resolve_dsh_home() -> Option<PathBuf> {
    dsh_home_from(|key| std::env::var(key).ok())
}

pub fn profile_dir(home: &Path) -> PathBuf {
    home.join("profiles").join(PROFILE_NAME)
}

pub fn patch_file(profile_dir: &Path) -> PathBuf {
    profile_dir.join(PROFILE_PATCH_FILENAME)
}
```

- [ ] **Step 5: 跑测试确认通过**

```bash
cd src-tauri && cargo test -p dsh-profile
```

预期：5 个测试 PASS。

- [ ] **Step 6: Commit**

```bash
git add src-tauri/Cargo.toml src-tauri/crates/dsh-profile
git commit -m "feat(profile): dsh-profile crate —— DSH_HOME 解析与 profile 路径定位"
```

---

### Task 6: patch 托管行 + `plugin_set_enabled` 命令 —— M2 后端

**Files:**
- Create: `src-tauri/crates/dsh-profile/src/patch.rs`
- Modify: `src-tauri/crates/dsh-profile/src/lib.rs`
- Modify: `src-tauri/src/lib.rs`

**Interfaces:**
- Consumes: Task 5 的 `home` 模块。
- Produces:
  - `dsh_profile::patch::set_disabled(patch_path: &Path, entry_id: &str, disabled: bool) -> Result<(), PatchError>`
  - `dsh_profile::patch::apply_set_disabled(text, entry_id, disabled) -> String`（纯函数，单测主战场）
  - `dsh_profile::patch::MARKER`（`"# dsh-client"`）、`PatchError`（Display：`{path}：{message}`）
  - Tauri 命令 `plugin_set_enabled({ entryId, disabled }) -> ()`（前端 Task 7 的 `setPluginEnabled` 依赖）

- [ ] **Step 1: 写失败测试**

`src-tauri/crates/dsh-profile/src/patch.rs` 先只放测试：

```rust
//! Text-level managed lines in the user patch layer (`cordis.patch.yml`).
//!
//! The file may contain `!!js` expressions, so it is never YAML-parsed or
//! evaluated. We only touch lines carrying our own end-of-line marker.

#[cfg(test)]
mod tests {
    use super::{apply_set_disabled, set_disabled, validate_entry_id, BACKUP_SUFFIX};
    use std::path::PathBuf;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dsh-profile-test-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn apply_to_empty_text_writes_one_block() {
        let out = apply_set_disabled("", "assistant/memory", true);
        assert_eq!(out, "- id: assistant/memory  # dsh-client\n  disabled: true  # dsh-client\n");
    }

    #[test]
    fn apply_is_idempotent() {
        let once = apply_set_disabled("", "a", true);
        assert_eq!(apply_set_disabled(&once, "a", true), once);
    }

    #[test]
    fn enable_writes_explicit_disabled_false() {
        let out = apply_set_disabled("", "a", false);
        assert!(out.contains("disabled: false"), "enable 也写显式行：{out}");
    }

    #[test]
    fn preserves_foreign_lines_including_bang_bang_js() {
        let foreign = "- insert:\n    - id: x\n      name: '@scope/x'\n      disabled: !!js process.platform === 'win32'\n- id: y\n  config:\n    k: 1\n";
        let out = apply_set_disabled(foreign, "a", true);
        assert!(out.starts_with(foreign), "他行原样保留：{out}");
        assert!(out.contains("- id: a  # dsh-client"));
    }

    #[test]
    fn flips_existing_block_without_duplicating() {
        let first = apply_set_disabled("", "a", true);
        let flipped = apply_set_disabled(&first, "a", false);
        assert_eq!(flipped.matches("- id: a  # dsh-client").count(), 1, "同一 entry 恰好一条：{flipped}");
        assert!(flipped.contains("disabled: false"));
    }

    #[test]
    fn entry_a_does_not_match_nested_a_b() {
        let text = "- id: a:b  # dsh-client\n  disabled: true  # dsh-client\n";
        let out = apply_set_disabled(text, "a", false);
        assert!(out.contains("- id: a:b  # dsh-client"), "前缀不误伤嵌套 id：{out}");
    }

    #[test]
    fn drops_stale_disabled_continuation_of_same_entry() {
        // 手工改乱过的文件：同一 entry 两条我方块，应收敛为一条。
        let messy = "- id: a  # dsh-client\n  disabled: true  # dsh-client\n- id: b\n  disabled: true\n- id: a  # dsh-client\n  disabled: true  # dsh-client\n";
        let out = apply_set_disabled(messy, "a", false);
        assert_eq!(out.matches("- id: a  # dsh-client").count(), 1);
        assert!(out.contains("- id: b\n  disabled: true\n"), "非我方行不动：{out}");
    }

    #[test]
    fn validate_entry_id_rejects_injection_chars() {
        for bad in ["", " a", "a ", "a\nb", "a\rb", "a#b"] {
            assert!(validate_entry_id(bad).is_err(), "应拒绝 {bad:?}");
        }
        assert!(validate_entry_id("a:b/c-d_e").is_ok());
    }

    #[test]
    fn set_disabled_creates_file_backup_and_is_atomic() {
        let dir = temp_dir("write");
        let patch = dir.join("cordis.patch.yml");
        std::fs::write(&patch, "- insert:\n    - id: x\n").unwrap();

        set_disabled(&patch, "a", true).unwrap();
        let text = std::fs::read_to_string(&patch).unwrap();
        assert!(text.contains("- id: a  # dsh-client"));

        let backup = dir.join(format!("cordis.patch.yml{BACKUP_SUFFIX}"));
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), "- insert:\n    - id: x\n", "首写备份为原始内容");

        // 第二次写入不覆盖备份。
        std::fs::write(&backup, "手工改过的备份").unwrap();
        set_disabled(&patch, "a", false).unwrap();
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), "手工改过的备份");
    }

    #[test]
    fn set_disabled_creates_missing_file_without_backup() {
        let dir = temp_dir("create");
        let patch = dir.join("cordis.patch.yml");
        set_disabled(&patch, "a", true).unwrap();
        assert!(patch.is_file());
        assert!(!dir.join(format!("cordis.patch.yml{BACKUP_SUFFIX}")).exists(), "无原件则无备份");
    }

    #[test]
    fn set_disabled_guides_when_profile_dir_missing() {
        let dir = temp_dir("missing");
        let patch = dir.join("nope").join("cordis.patch.yml");
        let err = set_disabled(&patch, "a", true).unwrap_err();
        assert!(err.to_string().contains("先启动一次 daemon"), "{err}");
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

```bash
cd src-tauri && cargo test -p dsh-profile
```

预期：编译失败。

- [ ] **Step 3: 实现 patch.rs（放在测试模块之前）**

```rust
//! Text-level managed lines in the user patch layer (`cordis.patch.yml`).
//!
//! The file may contain `!!js` expressions, so it is never YAML-parsed or
//! evaluated. We only touch lines carrying our own end-of-line marker, and
//! every write is explicit intent: drop all our blocks for the entry, then
//! append exactly one.

use std::path::{Path, PathBuf};

/// End-of-line marker identifying every line this client owns.
pub const MARKER: &str = "# dsh-client";
/// First-write backup suffix, created once and never overwritten.
pub const BACKUP_SUFFIX: &str = ".dsh-client.bak";
/// Atomic-write scratch suffix, renamed over the target.
const TMP_SUFFIX: &str = ".dsh-client.tmp";

/// Patch write failure: message names the path and the cause.
#[derive(Debug)]
pub struct PatchError {
    pub path: PathBuf,
    pub message: String,
}

impl std::fmt::Display for PatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}：{}", self.path.display(), self.message)
    }
}

impl std::error::Error for PatchError {}

/// Reject entry ids that could break the two-line managed block shape or
/// smuggle YAML into the file.
pub fn validate_entry_id(entry_id: &str) -> Result<(), String> {
    if entry_id.is_empty() || entry_id.trim() != entry_id {
        return Err(format!("entryId 为空或含首尾空白：{entry_id:?}"));
    }
    if entry_id.contains(['\n', '\r', '#']) {
        return Err(format!("entryId 含非法字符：{entry_id:?}"));
    }
    Ok(())
}

fn is_marked(line: &str) -> bool {
    line.trim_end().ends_with(MARKER)
}

/// `- id: <entryId>  # dsh-client`，标记前的间距不敏感，id 精确匹配
/// （`a` 不匹配 `a:b`）。
fn is_managed_id_line(line: &str, entry_id: &str) -> bool {
    let Some(head) = line.trim_end().strip_suffix(MARKER) else {
        return false;
    };
    head.trim_end() == format!("- id: {entry_id}")
}

/// Apply the explicit intent "entry `entry_id` disabled = `disabled`" to
/// patch file text, returning the new text. Idempotent.
pub fn apply_set_disabled(text: &str, entry_id: &str, disabled: bool) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        if is_managed_id_line(line, entry_id) {
            // Drop the id line plus our marked continuation lines. Ours are
            // indented and never start a fresh `- ` item, so a foreign entry
            // cannot be eaten.
            while let Some(next) = lines.peek() {
                if is_marked(next) && !next.trim_start().starts_with("- ") {
                    lines.next();
                } else {
                    break;
                }
            }
            continue;
        }
        out.push(line);
    }
    let mut result = if out.is_empty() { String::new() } else { out.join("\n") + "\n" };
    result.push_str(&format!("- id: {entry_id}  {MARKER}\n  disabled: {disabled}  {MARKER}\n"));
    result
}

fn sibling_with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("cordis.patch.yml");
    path.with_file_name(format!("{name}{suffix}"))
}

/// Write the explicit intent to the patch file at `patch_path`.
///
/// - Missing parent directory → guidance error (profile not initialized).
/// - First write copies the file to `<file>.dsh-client.bak` (never overwritten).
/// - Atomic: scratch file + rename, mirroring upstream include's writeback.
/// - No-op when the intent is already realized (keeps mtime stable, so the
///   upstream HMR watcher does not see phantom writes).
pub fn set_disabled(patch_path: &Path, entry_id: &str, disabled: bool) -> Result<(), PatchError> {
    let err = |message: String| PatchError { path: patch_path.to_path_buf(), message };
    validate_entry_id(entry_id).map_err(&err)?;

    let parent = patch_path.parent().expect("patch file path has a parent");
    if !parent.is_dir() {
        return Err(err("profile 目录不存在；请先启动一次 daemon 以初始化 profile".into()));
    }

    let (original, existed) = match std::fs::read_to_string(patch_path) {
        Ok(text) => (text, true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (String::new(), false),
        Err(e) => return Err(err(format!("读取失败：{e}"))),
    };

    let next = apply_set_disabled(&original, entry_id, disabled);
    if next == original {
        return Ok(());
    }

    if existed {
        let backup = sibling_with_suffix(patch_path, BACKUP_SUFFIX);
        if !backup.exists() {
            std::fs::copy(patch_path, &backup).map_err(|e| err(format!("备份失败：{e}")))?;
        }
    }

    let tmp = sibling_with_suffix(patch_path, TMP_SUFFIX);
    std::fs::write(&tmp, next).map_err(|e| err(format!("写入临时文件失败：{e}")))?;
    std::fs::rename(&tmp, patch_path).map_err(|e| err(format!("原子替换失败：{e}")))?;
    Ok(())
}
```

`src-tauri/crates/dsh-profile/src/lib.rs` 改为：

```rust
//! dsh-profile — filesystem and CLI seams of a dsh profile.
//!
//! Everything here is plain logic over paths/text/processes with zero Tauri
//! imports, so it is unit-testable end to end.

pub mod home;
pub mod patch;
```

- [ ] **Step 4: 跑测试确认通过**

```bash
cd src-tauri && cargo test -p dsh-profile
```

预期：home 5 个 + patch 10 个测试全 PASS。

- [ ] **Step 5: 加 `plugin_set_enabled` 命令**

`src-tauri/src/lib.rs` 追加并注册：

```rust
/// Write one managed patch line pair (`disabled: true|false`) for the entry.
/// Hot application is upstream's HMR; the frontend verifies by polling.
#[tauri::command]
fn plugin_set_enabled(entry_id: String, disabled: bool) -> Result<(), String> {
    let home = dsh_profile::home::resolve_dsh_home()
        .ok_or_else(|| "无法定位 dsh home（DSH_HOME 未设置且无法解析用户目录）".to_string())?;
    let patch = dsh_profile::home::patch_file(&dsh_profile::home::profile_dir(&home));
    dsh_profile::patch::set_disabled(&patch, &entry_id, disabled).map_err(|err| err.to_string())
}
```

（Tauri 自动把 JS 的 `entryId` 映射到 Rust 的 `entry_id`。）

- [ ] **Step 6: 编译 + 全量测试 + Commit**

```bash
cd src-tauri && cargo check && cargo test
git add src-tauri/crates/dsh-profile src-tauri/src/lib.rs
git commit -m "feat(profile): patch 托管行写入 + plugin_set_enabled 命令 —— M2 后端"
```

---

### Task 7: 前端启用/禁用（开关 + 轮询验证 + 超时提示）—— M2 完成

**Files:**
- Modify: `src/plugins/plugins.ts`
- Modify: `src/plugins/PluginsApp.vue`
- Modify: `src/plugins/InventoryList.vue`（整个替换为开关版）
- Modify: `src/styles.css`

**Interfaces:**
- Consumes: Task 6 的 `plugin_set_enabled`；Task 4 的 `loadInventories`/`entries`。
- Produces: `setPluginEnabled(entryId, enabled)`；`PendingIntent`；PluginsApp 的 `pending`/`notice`/`showNotice(text)`/`onToggle(entry)`（Task 11/13/14 复用 `showNotice`）。

- [ ] **Step 1: plugins.ts 追加**

```ts
// ---- 启用/禁用（patch 托管行）----

export interface PendingIntent {
  intent: boolean
  deadline: number
}

/** enabled 是 UI 语义；patch 层写的是 disabled 标志，取反。 */
export const setPluginEnabled = (entryId: string, enabled: boolean): Promise<void> =>
  invoke('plugin_set_enabled', { entryId, disabled: !enabled })
```

- [ ] **Step 2: PluginsApp.vue 叠加 pending/notice**

`<script setup>` 的 import 区给 `./plugins` 那条加上 `setPluginEnabled`，类型那条加上 `PendingIntent`：

```ts
import { fetchDynamicInventory, fetchInventory, setPluginEnabled, usePolling } from './plugins'
import type { DynamicPluginRow, PendingIntent, PluginEntry } from './plugins'
```

`loadError` 声明之后追加：

```ts
const pending = ref<Record<string, PendingIntent>>({})
const notice = ref<string | null>(null)
let noticeTimer: number | undefined

function showNotice(text: string): void {
  notice.value = text
  window.clearTimeout(noticeTimer)
  noticeTimer = window.setTimeout(() => {
    notice.value = null
  }, 6000)
}

async function onToggle(entry: PluginEntry): Promise<void> {
  const intent = !entry.enabled
  try {
    await setPluginEnabled(entry.entryId, intent)
    pending.value = { ...pending.value, [entry.entryId]: { intent, deadline: Date.now() + 10_000 } }
  } catch (err) {
    showNotice(`写入开关失败：${String(err)}`)
  }
}

/** 每轮轮询后核对未决意图：生效即清除；10s 未生效提示 HMR 可能未应用。 */
function settlePending(): void {
  if (!Object.keys(pending.value).length) return
  const now = Date.now()
  const next = { ...pending.value }
  for (const [entryId, p] of Object.entries(next)) {
    const current = entries.value.find((e) => e.entryId === entryId)
    if (current && current.enabled === p.intent) {
      delete next[entryId]
    } else if (now > p.deadline) {
      delete next[entryId]
      showNotice(`插件 ${entryId} 的开关未在 10 秒内生效（HMR 可能未应用），可尝试重启 daemon`)
    }
  }
  pending.value = next
}
```

`loadInventories` 里 `entries.value = snap.entries` 之后加一行 `settlePending()`。

模板 `<header class="bar">…</header>` 之后加：

```html
    <p v-if="notice" class="notice">{{ notice }}</p>
```

`InventoryList` 标签改为：

```html
        <InventoryList v-if="tab === 'inventory'" :entries="entries" :pending="pending" @toggle="onToggle" />
```

- [ ] **Step 3: InventoryList.vue 整个替换为开关版**

```vue
<script setup lang="ts">
import type { PendingIntent, PluginEntry } from './plugins'

const props = defineProps<{
  entries: PluginEntry[]
  pending: Record<string, PendingIntent>
}>()

const emit = defineEmits<{ toggle: [entry: PluginEntry] }>()

function phaseLabel(entry: PluginEntry): string {
  switch (entry.fiberPhase) {
    case 'pending':
      return '等待加载'
    case 'loading':
      return '加载中'
    case 'active':
      return '运行中'
    case 'failed':
      return '加载失败'
    case 'unloading':
      return '卸载中'
    case null:
      return '未加载'
  }
}

function isPending(entry: PluginEntry): boolean {
  return props.pending[entry.entryId] !== undefined
}
</script>

<template>
  <div class="inventory">
    <p v-if="!entries.length" class="waiting">清单为空。</p>
    <table v-else class="table">
      <thead>
        <tr>
          <th>插件</th>
          <th>模块</th>
          <th>状态</th>
          <th>启用</th>
        </tr>
      </thead>
      <tbody>
        <tr v-for="entry in entries" :key="entry.entryId">
          <td class="mono">{{ entry.entryId }}</td>
          <td class="mono dim">{{ entry.moduleName }}</td>
          <td><span class="phase" :data-phase="entry.fiberPhase ?? 'none'">{{ phaseLabel(entry) }}</span></td>
          <td>
            <button
              :class="['switch', { on: entry.enabled }]"
              :disabled="isPending(entry)"
              :title="isPending(entry) ? '等待生效…' : entry.enabled ? '点击禁用' : '点击启用'"
              @click="emit('toggle', entry)"
            >
              {{ isPending(entry) ? '…' : entry.enabled ? '启用' : '禁用' }}
            </button>
          </td>
        </tr>
      </tbody>
    </table>
  </div>
</template>
```

- [ ] **Step 4: styles.css 追加开关样式**

```css
.switch {
  min-width: 56px;
  padding: 3px 10px;
  border-radius: 999px;
  border: 1px solid var(--border);
  background: none;
  color: var(--fg-dim);
  font-size: 12px;
  cursor: pointer;
}

.switch.on {
  color: #6fd38c;
  border-color: #2c5f3d;
  background: rgba(111, 211, 140, 0.08);
}

.switch:disabled {
  opacity: 0.5;
  cursor: wait;
}
```

- [ ] **Step 5: 验证（M2 验收）**

```bash
pnpm build
```

手动验收（同 M1 的启动命令）：插件窗口 → 清单任一条目点开关 → 按钮进入"…"待生效态 → 3s 内轮询确认翻转 → 再点回来。另验：直接文本打开 `%USERPROFILE%\.dsh\profiles\web\cordis.patch.yml`，看到带 `# dsh-client` 标记的两行块；存在 `.dsh-client.bak` 备份。

- [ ] **Step 6: Commit**

```bash
git add src/plugins/ src/styles.css
git commit -m "feat: 插件启用/禁用 —— 开关 + 待生效态 + 10s 轮询验证 —— M2 完成"
```

---

### Task 8: supervisor resolve 拆分（供 CLI 任务复用）

**Files:**
- Modify: `src-tauri/crates/dsh-supervisor/src/resolve.rs`
- Modify: `src-tauri/crates/dsh-supervisor/src/lib.rs`（re-export）

**Interfaces:**
- Consumes: 无（纯重构 + 新增）。
- Produces:
  - `dsh_supervisor::DshInvocation { program: String, prefix: Vec<String> }`（`Clone + Debug`）
  - `dsh_supervisor::resolve_invocation(bin_override: Option<&str>) -> DshInvocation`
  - `DshInvocation::plan(&self, args: &[&str]) -> LaunchPlan`、`DshInvocation::display(&self) -> String`
  - `LaunchPlan::spawn_env(&self, extra_env: &[(&str, &str)]) -> io::Result<Child>`（`spawn()` 变为 `spawn_env(&[])` 的转发）
  - 既有 `resolve_launch` / `LaunchPlan::spawn` 签名与行为不变（Task 9 的消费方是 `dsh-profile`）。

- [ ] **Step 1: 写失败测试**

`src-tauri/crates/dsh-supervisor/src/resolve.rs` 末尾追加测试模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_splits_program_and_prefix() {
        let invocation = resolve_invocation(Some("node C:\\nvm\\v24.15.0\\bin.js"));
        assert_eq!(invocation.program, "node");
        assert_eq!(invocation.prefix, vec!["C:\\nvm\\v24.15.0\\bin.js"]);
    }

    #[test]
    fn blank_override_falls_through_without_panic() {
        // PATH 分支依赖运行机器，只断言程序名非空。
        assert!(!resolve_invocation(Some("   ")).program.is_empty());
    }

    #[test]
    fn plan_appends_subcommand_args() {
        let invocation = resolve_invocation(Some("node /opt/dsh/bin.js"));
        let plan = invocation.plan(&["plugin", "--profile", "web", "add", "foo"]);
        assert_eq!(plan.program, "node");
        assert_eq!(plan.args, vec!["/opt/dsh/bin.js", "plugin", "--profile", "web", "add", "foo"]);
    }

    #[test]
    fn resolve_launch_keeps_legacy_shape() {
        let plan = resolve_launch(Some("node /opt/dsh/bin.js"));
        assert_eq!(plan.args, vec!["/opt/dsh/bin.js", "web", "--port", "0"]);
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

```bash
cd src-tauri && cargo test -p dsh-supervisor
```

预期：编译失败（`resolve_invocation` 未定义）。

- [ ] **Step 3: 重构 resolve.rs**

文件头注释更新为：

```rust
//! Launch-plan resolution: `DSH_CLIENT_BIN` override, then `dsh` on PATH,
//! then `npx -y @deepseek-ai/dsh@latest`. [`DshInvocation`] is the resolved
//! "how to reach the dsh CLI" (program plus binary-selecting prefix);
//! [`LaunchPlan`] is one concrete argv built from it. Spawning is the only
//! side effect, and it lives in [`LaunchPlan::spawn_env`].
```

`LaunchPlan` 之后插入 `DshInvocation`：

```rust
/// How to reach the dsh CLI on this machine: program plus the argument
/// prefix that selects the dsh binary (override tokens / npx package spec).
/// The subcommand and its args are appended by the caller.
#[derive(Debug, Clone)]
pub struct DshInvocation {
    pub program: String,
    pub prefix: Vec<String>,
}

impl DshInvocation {
    /// Build one concrete argv, e.g. `plan(&["web", "--port", "0"])`.
    pub fn plan(&self, args: &[&str]) -> LaunchPlan {
        let mut full = self.prefix.clone();
        full.extend(args.iter().map(|s| (*s).to_string()));
        LaunchPlan { program: self.program.clone(), args: full }
    }

    /// One-line display for diagnostics; never executed.
    pub fn display(&self) -> String {
        format!("{} {}", self.program, self.prefix.join(" "))
    }
}
```

`LaunchPlan` 的 `spawn` 改为转发，新增 `spawn_env`（原 `spawn` 函数体原样搬入，只在 `cmd.args(&self.args)…` 链之后加 env 循环）：

```rust
impl LaunchPlan {
    /// One-line display for diagnostics; never executed.
    pub fn display(&self) -> String {
        format!("{} {}", self.program, self.args.join(" "))
    }

    /// Spawn with piped stdout/stderr and a killed-with-us lifetime.
    pub async fn spawn(&self) -> std::io::Result<Child> {
        self.spawn_env(&[]).await
    }

    /// Spawn with extra environment (`CI=true` keeps pnpm non-interactive).
    pub async fn spawn_env(&self, extra_env: &[(&str, &str)]) -> std::io::Result<Child> {
        // npm shims on Windows are .cmd files: only cmd.exe can run them.
        #[cfg(windows)]
        let mut cmd = {
            let mut c = Command::new("cmd");
            c.arg("/c").arg(&self.program);
            c
        };
        #[cfg(unix)]
        let mut cmd = Command::new(&self.program);

        cmd.args(&self.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in extra_env {
            cmd.env(key, value);
        }
        // Dedicated npm cache: the machine-wide `_npx` cache is shared with
        // every other npm user on the host, and concurrent installs racing
        // on it abort with EPERM on Windows. The daemon gets its own.
        if std::env::var_os("npm_config_cache").is_none() {
            if let Ok(local) = std::env::var("LOCALAPPDATA") {
                let cache = std::path::Path::new(&local)
                    .join("dsh-client")
                    .join("npm-cache");
                cmd.env("npm_config_cache", &cache);
            }
        }
        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);
        #[cfg(unix)]
        cmd.process_group(0);
        cmd.spawn()
    }
}
```

`resolve_launch` 改为委托，新增 `resolve_invocation`：

```rust
/// Resolve how this machine reaches the dsh CLI.
pub fn resolve_invocation(bin_override: Option<&str>) -> DshInvocation {
    if let Some(bin) = bin_override.map(str::trim).filter(|s| !s.is_empty()) {
        let mut parts = bin.split_whitespace();
        let program = parts.next().unwrap_or("dsh").to_string();
        let prefix: Vec<String> = parts.map(String::from).collect();
        return DshInvocation { program, prefix };
    }
    if find_in_path("dsh").is_some() {
        return DshInvocation { program: "dsh".into(), prefix: Vec::new() };
    }
    if find_in_path("npx").is_some() {
        return DshInvocation {
            program: "npx".into(),
            prefix: ["-y", "@deepseek-ai/dsh@latest"].iter().map(|s| s.to_string()).collect(),
        };
    }
    // Nothing on PATH: keep the plain name so the spawn error names the target.
    DshInvocation { program: "dsh".into(), prefix: Vec::new() }
}

/// Resolve how this machine launches `dsh web`.
pub fn resolve_launch(bin_override: Option<&str>) -> LaunchPlan {
    resolve_invocation(bin_override).plan(&["web", "--port", "0"])
}
```

`find_in_path` / `extract_bin_token` / `find_first_node_in_path` 保持原样。

`src-tauri/crates/dsh-supervisor/src/lib.rs` 的 re-export 改为：

```rust
pub use resolve::{
    extract_bin_token, find_first_node_in_path, resolve_invocation, resolve_launch, DshInvocation,
    LaunchPlan,
};
```

- [ ] **Step 4: 跑测试确认通过（既有测试全绿）**

```bash
cd src-tauri && cargo test
```

预期：workspace 全部测试 PASS（含 supervisor 既有测试与新 4 个）。

- [ ] **Step 5: Commit**

```bash
git add src-tauri/crates/dsh-supervisor
git commit -m "refactor(supervisor): 拆出 DshInvocation 与 spawn_env，供 dsh plugin CLI 复用（行为不变）"
```

---

### Task 9: `PluginTaskRunner` 串行任务队列

**Files:**
- Create: `src-tauri/crates/dsh-profile/src/tasks.rs`
- Modify: `src-tauri/crates/dsh-profile/src/lib.rs`

**Interfaces:**
- Consumes: Task 8 的 `DshInvocation` / `LaunchPlan::spawn_env`。
- Produces:
  - `dsh_profile::tasks::PluginTaskRunner`（`Clone`；`start(invocation) -> Self` 在当前 tokio runtime spawn 串行 worker；`submit(kind, spec) -> Result<String, String>` 返回 taskId；`list() -> Vec<PluginTaskView>`；`subscribe() -> broadcast::Receiver<PluginTaskView>`）
  - `PluginTaskKind { Install, Remove }`、`PluginTaskStatus { Running, Done, Failed }`、`PluginTaskView { taskId, kind, spec, status, outputTail, exitCode }`（serde camelCase，即 `plugins://task` 事件 payload 形状，Task 10/11 依赖）
  - 常量 `OUTPUT_TAIL_CAP = 500`

- [ ] **Step 1: 写失败测试**

`src-tauri/crates/dsh-profile/src/tasks.rs` 先放测试（串行性是本 crate 的核心承诺，用假命令端到端验）：

```rust
//! Serial runner for `dsh plugin --profile web add|remove <spec>`.
//!
//! pnpm holds a lock on the profile directory, so tasks run strictly one at
//! a time; output streams into a bounded tail and every state change is
//! broadcast for the UI.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_spec_rejects_shell_metacharacters() {
        // Windows 下经 cmd /c 透传，& | < > ^ " 与空白都可能被 cmd 重新解释。
        for bad in ["", " a", "a ", "a b", "a&calc", "a|b", "a>b", "a\"b", "a^b", "a\nb"] {
            assert!(validate_spec(bad).is_err(), "应拒绝 {bad:?}");
        }
        assert!(validate_spec("@scope/name@^1.2.3").is_ok());
        assert!(validate_spec("github:user/repo").is_ok());
    }

    #[test]
    fn output_tail_is_capped() {
        let mut view = PluginTaskView {
            task_id: "t".into(),
            kind: PluginTaskKind::Install,
            spec: "s".into(),
            status: PluginTaskStatus::Running,
            output_tail: Vec::new(),
            exit_code: None,
        };
        for i in 0..600 {
            push_line(&mut view, format!("line-{i}"));
        }
        assert_eq!(view.output_tail.len(), OUTPUT_TAIL_CAP);
        assert_eq!(view.output_tail[0], "line-100");
    }

    /// 假 dsh：把每次调用的 argv 追加到日志，中间睡一拍。两个任务排队后，
    /// 日志顺序必须是 [start a, end a, start b, end b]——第二个任务不得穿插。
    #[tokio::test(flavor = "multi_thread")]
    async fn tasks_run_strictly_serial() {
        let dir = std::env::temp_dir().join(format!("dsh-runner-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("calls.log");

        #[cfg(windows)]
        let program = {
            let script = dir.join("fake-dsh.cmd");
            std::fs::write(
                &script,
                format!(
                    "@echo off\r\necho start %* >> \"{}\"\r\nping -n 2 127.0.0.1 >nul\r\necho end %* >> \"{}\"\r\n",
                    log.display(),
                    log.display()
                ),
            )
            .unwrap();
            script.to_string_lossy().into_owned()
        };
        #[cfg(unix)]
        let program = {
            use std::os::unix::fs::PermissionsExt;
            let script = dir.join("fake-dsh.sh");
            std::fs::write(
                &script,
                format!("#!/bin/sh\necho start \"$@\" >> \"{}\"\nsleep 1\necho end \"$@\" >> \"{}\"\n", log.display(), log.display()),
            )
            .unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
            script.to_string_lossy().into_owned()
        };

        let runner = PluginTaskRunner::start(DshInvocation { program, prefix: vec![] });
        let t1 = runner.submit(PluginTaskKind::Install, "spec-a".into()).unwrap();
        let t2 = runner.submit(PluginTaskKind::Remove, "spec-b".into()).unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            let views = runner.list();
            let done = |id: &str| views.iter().any(|v| v.task_id == id && v.status == PluginTaskStatus::Done);
            if done(&t1) && done(&t2) {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "任务未在 15s 内完成：{:?}", runner.list());
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }

        let calls = std::fs::read_to_string(&log).unwrap();
        let order: Vec<&str> = calls
            .lines()
            .map(|l| if l.contains("spec-a") { "a" } else { "b" })
            .collect();
        assert_eq!(order, ["a", "a", "b", "b"], "串行执行：{calls}");
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

```bash
cd src-tauri && cargo test -p dsh-profile
```

预期：编译失败（`validate_spec` 等未定义）。

- [ ] **Step 3: 实现 tasks.rs（放在测试模块之前）**

```rust
//! Serial runner for `dsh plugin --profile web add|remove <spec>`.
//!
//! pnpm holds a lock on the profile directory, so tasks run strictly one at
//! a time; output streams into a bounded tail and every state change is
//! broadcast for the UI. Reuses the supervisor's launch resolution so the
//! CLI binary is the same one the daemon came from.

use std::sync::{Arc, Mutex};

use dsh_supervisor::DshInvocation;
use serde::Serialize;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{broadcast, mpsc};

/// Output ring capacity, aligned with dsh-supervisor's log tail.
pub const OUTPUT_TAIL_CAP: usize = 500;
/// Finished tasks retained for a late-joining window.
const FINISHED_RETAIN: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PluginTaskKind {
    Install,
    Remove,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PluginTaskStatus {
    Running,
    Done,
    Failed,
}

/// `plugins://task` event payload (serde camelCase on the wire).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginTaskView {
    pub task_id: String,
    pub kind: PluginTaskKind,
    pub spec: String,
    pub status: PluginTaskStatus,
    pub output_tail: Vec<String>,
    pub exit_code: Option<i32>,
}

struct Job {
    task_id: String,
    kind: PluginTaskKind,
    spec: String,
}

/// A spec is one argv token; on Windows it crosses `cmd /c`, which
/// re-parses the line — metacharacters are refused outright.
fn validate_spec(spec: &str) -> Result<(), String> {
    if spec.is_empty() || spec.trim() != spec {
        return Err("spec 为空或含首尾空白".to_string());
    }
    if spec.chars().any(|c| c.is_whitespace() || "&|<>^\"\r\n".contains(c)) {
        return Err(format!("spec 含非法字符：{spec:?}"));
    }
    Ok(())
}

fn push_line(view: &mut PluginTaskView, line: String) {
    view.output_tail.push(line);
    if view.output_tail.len() > OUTPUT_TAIL_CAP {
        let overflow = view.output_tail.len() - OUTPUT_TAIL_CAP;
        view.output_tail.drain(..overflow);
    }
}

#[derive(Clone)]
pub struct PluginTaskRunner {
    sender: mpsc::UnboundedSender<Job>,
    views: Arc<Mutex<Vec<PluginTaskView>>>,
    events: broadcast::Sender<PluginTaskView>,
}

impl PluginTaskRunner {
    /// Spawn the serial worker on the current tokio runtime.
    pub fn start(invocation: DshInvocation) -> Self {
        let (sender, receiver) = mpsc::unbounded_channel::<Job>();
        let (events, _) = broadcast::channel(64);
        let runner = Self { sender, views: Arc::new(Mutex::new(Vec::new())), events };
        let worker = runner.clone();
        tokio::spawn(async move { worker_loop(invocation, receiver, worker).await });
        runner
    }

    /// Queue one task; the task id returns immediately, progress comes via
    /// [`PluginTaskRunner::subscribe`].
    pub fn submit(&self, kind: PluginTaskKind, spec: String) -> Result<String, String> {
        validate_spec(&spec)?;
        let task_id = uuid::Uuid::new_v4().to_string();
        let view = PluginTaskView {
            task_id: task_id.clone(),
            kind,
            spec: spec.clone(),
            status: PluginTaskStatus::Running,
            output_tail: Vec::new(),
            exit_code: None,
        };
        self.views.lock().expect("views lock poisoned").push(view.clone());
        let _ = self.events.send(view);
        self.sender
            .send(Job { task_id: task_id.clone(), kind, spec })
            .map_err(|_| "任务队列已关闭".to_string())?;
        Ok(task_id)
    }

    pub fn list(&self) -> Vec<PluginTaskView> {
        self.views.lock().expect("views lock poisoned").clone()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<PluginTaskView> {
        self.events.subscribe()
    }

    fn with_view(&self, task_id: &str, f: impl FnOnce(&mut PluginTaskView)) {
        let view = {
            let mut views = self.views.lock().expect("views lock poisoned");
            let Some(view) = views.iter_mut().find(|v| v.task_id == task_id) else { return };
            f(view);
            view.clone()
        };
        let _ = self.events.send(view);
    }

    fn finish(&self, task_id: &str, status: PluginTaskStatus, exit_code: Option<i32>, line: String) {
        self.with_view(task_id, |view| {
            push_line(view, line);
            view.status = status;
            view.exit_code = exit_code;
        });
        // Retention: bound finished tasks, oldest first.
        let mut views = self.views.lock().expect("views lock poisoned");
        let finished = views.iter().filter(|v| v.status != PluginTaskStatus::Running).count();
        let mut remove = finished.saturating_sub(FINISHED_RETAIN);
        if remove > 0 {
            views.retain(|v| {
                if remove > 0 && v.status != PluginTaskStatus::Running {
                    remove -= 1;
                    false
                } else {
                    true
                }
            });
        }
    }
}

/// Stream one piped child output into the task's tail, line by line.
fn spawn_reader<S>(stream: S, runner: PluginTaskRunner, task_id: String) -> tokio::task::JoinHandle<()>
where
    S: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(stream).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            runner.with_view(&task_id, |view| push_line(view, line));
        }
    })
}

async fn worker_loop(
    invocation: DshInvocation,
    mut receiver: mpsc::UnboundedReceiver<Job>,
    runner: PluginTaskRunner,
) {
    while let Some(job) = receiver.recv().await {
        run_one(&invocation, &job, &runner).await;
    }
}

async fn run_one(invocation: &DshInvocation, job: &Job, runner: &PluginTaskRunner) {
    let verb = match job.kind {
        PluginTaskKind::Install => "add",
        PluginTaskKind::Remove => "remove",
    };
    let plan = invocation.plan(&["plugin", "--profile", "web", verb, &job.spec]);
    runner.with_view(&job.task_id, |view| push_line(view, format!("$ {}", plan.display())));

    let spawned = plan.spawn_env(&[("CI", "true")]).await;
    let mut child = match spawned {
        Ok(child) => child,
        Err(err) => {
            runner.finish(&job.task_id, PluginTaskStatus::Failed, None, format!("启动失败：{err}"));
            return;
        }
    };

    // stdout/stderr 类型不同（ChildStdout/ChildStderr），用泛型 reader。
    let stdout = child.stdout.take().expect("spawn pipes stdout");
    let stderr = child.stderr.take().expect("spawn pipes stderr");
    let h1 = spawn_reader(stdout, runner.clone(), job.task_id.clone());
    let h2 = spawn_reader(stderr, runner.clone(), job.task_id.clone());
    let _ = tokio::join!(h1, h2);

    match child.wait().await {
        Ok(status) if status.success() => {
            runner.finish(&job.task_id, PluginTaskStatus::Done, status.code(), "完成".into());
        }
        Ok(status) => {
            runner.finish(&job.task_id, PluginTaskStatus::Failed, status.code(), format!("退出码 {:?}", status.code()));
        }
        Err(err) => {
            runner.finish(&job.task_id, PluginTaskStatus::Failed, None, format!("等待退出失败：{err}"));
        }
    }
}
```

`src-tauri/crates/dsh-profile/src/lib.rs` 加：

```rust
pub mod tasks;
```

- [ ] **Step 4: 跑测试确认通过**

```bash
cd src-tauri && cargo test -p dsh-profile
```

预期：3 个新测试 PASS（串行测试约 2s）。

- [ ] **Step 5: Commit**

```bash
git add src-tauri/crates/dsh-profile
git commit -m "feat(profile): PluginTaskRunner —— dsh plugin CLI 串行队列 + 输出尾环 + 状态广播"
```

---

### Task 10: 安装/移除命令 + `plugins://task` 转发

**Files:**
- Modify: `src-tauri/src/lib.rs`

**Interfaces:**
- Consumes: Task 9 的 `PluginTaskRunner`；setup 里的 `bin_override`。
- Produces: Tauri 命令 `plugin_install({ spec }) -> taskId`、`plugin_remove({ spec }) -> taskId`；事件 `plugins://task`（payload = `PluginTaskView`）。前端 Task 11 依赖。

- [ ] **Step 1: 加状态、命令与转发任务**

`src-tauri/src/lib.rs` 顶部 use 区追加：

```rust
use dsh_profile::tasks::{PluginTaskKind, PluginTaskRunner};
```

`DaemonState` 之后追加：

```rust
/// Plugin install/remove task queue. Independent of preflight: the CLI talks
/// to the profile directory, not the daemon.
#[derive(Clone)]
struct PluginTasksState {
    runner: PluginTaskRunner,
}
```

`daemon_stop` 之后追加：

```rust
#[tauri::command]
fn plugin_install(state: tauri::State<'_, PluginTasksState>, spec: String) -> Result<String, String> {
    state.runner.submit(PluginTaskKind::Install, spec)
}

#[tauri::command]
fn plugin_remove(state: tauri::State<'_, PluginTasksState>, spec: String) -> Result<String, String> {
    state.runner.submit(PluginTaskKind::Remove, spec)
}
```

`forward_task` 之后追加：

```rust
/// Forward `PluginTaskView` updates as `plugins://task` (all windows; the
/// plugins window is the only listener).
async fn plugins_forward_task(app: AppHandle, runner: PluginTaskRunner) {
    let mut receiver = runner.subscribe();
    loop {
        match receiver.recv().await {
            Ok(view) => {
                let _ = app.emit("plugins://task", &view);
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(count)) => {
                // Full-view events are cumulative, so a skipped intermediate
                // state is harmless.
                warn!(count, "plugins://task lag");
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        }
    }
}
```

`setup` 闭包里 `app.manage(PreflightState { … })` 之后追加：

```rust
            let invocation = dsh_supervisor::resolve_invocation(bin_override.as_deref());
            let runner = tauri::async_runtime::block_on(async { PluginTaskRunner::start(invocation) });
            app.manage(PluginTasksState { runner: runner.clone() });
            tauri::async_runtime::spawn(plugins_forward_task(app.handle().clone(), runner));
```

`invoke_handler` 注册 `plugin_install, plugin_remove`。

- [ ] **Step 2: 编译 + 全量测试**

```bash
cd src-tauri && cargo check && cargo test
```

预期：全绿。

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/lib.rs
git commit -m "feat: plugin_install/plugin_remove 命令 + plugins://task 事件转发"
```

---

### Task 11: 安装/移除对话框 + 任务中心 —— M3 完成

**Files:**
- Modify: `src/plugins/plugins.ts`
- Create: `src/plugins/InstallDialog.vue`
- Create: `src/plugins/TaskCenter.vue`
- Modify: `src/plugins/PluginsApp.vue`
- Modify: `src/styles.css`

**Interfaces:**
- Consumes: Task 10 的命令与事件；`daemon.ts` 既有 `restartDaemon`；Task 7 的 `showNotice`。
- Produces: `installPlugin(spec) / removePlugin(spec) -> Promise<string>`；`PluginTaskView`（TS 镜像）；对话框 `mode: 'install' | 'remove'`。

- [ ] **Step 1: plugins.ts 追加**

```ts
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
```

- [ ] **Step 2: 写 InstallDialog.vue**

```vue
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
        :placeholder="mode === 'install' ? 'npm 包 spec，如 @scope/name@^1.0.0 或 github:user/repo' : '包名，如 @scope/name'"
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
```

- [ ] **Step 3: 写 TaskCenter.vue**

```vue
<script setup lang="ts">
import { onMounted, onUnmounted, ref } from 'vue'
import { listen } from '@tauri-apps/api/event'
import type { UnlistenFn } from '@tauri-apps/api/event'
import { restartDaemon } from '../daemon'
import type { PluginTaskView } from './plugins'

const emit = defineEmits<{ notice: [text: string] }>()

const tasks = ref<PluginTaskView[]>([])
const restarting = ref(false)
let unlisten: UnlistenFn | undefined

onMounted(async () => {
  unlisten = await listen<PluginTaskView>('plugins://task', (event) => {
    const view = event.payload
    const index = tasks.value.findIndex((t) => t.taskId === view.taskId)
    if (index >= 0) tasks.value[index] = view
    else tasks.value.push(view)
  })
})

onUnmounted(() => {
  unlisten?.()
})

function tail(task: PluginTaskView): string {
  return task.outputTail.slice(-8).join('\n')
}

function statusLabel(task: PluginTaskView): string {
  switch (task.status) {
    case 'running':
      return '执行中'
    case 'done':
      return '完成'
    case 'failed':
      return `失败（退出码 ${task.exitCode ?? '？'}）`
  }
}

async function restart(): Promise<void> {
  restarting.value = true
  try {
    await restartDaemon()
    emit('notice', 'daemon 正在重启，就绪后插件变更生效')
  } catch (err) {
    emit('notice', `重启失败：${String(err)}`)
  } finally {
    restarting.value = false
  }
}
</script>

<template>
  <div class="tasks">
    <p v-if="!tasks.length" class="waiting">还没有安装/移除任务。</p>
    <article v-for="task in tasks" :key="task.taskId" class="task-card" :data-status="task.status">
      <header>
        <span v-if="task.status === 'running'" class="spinner"></span>
        <strong>{{ task.kind === 'install' ? '安装' : '移除' }} {{ task.spec }}</strong>
        <span class="phase">{{ statusLabel(task) }}</span>
      </header>
      <pre v-if="task.outputTail.length" class="task-out">{{ tail(task) }}</pre>
      <footer v-if="task.status === 'done'" class="done-banner">
        <span>需重启 daemon 后生效。</span>
        <button class="mini primary" :disabled="restarting" @click="restart">
          {{ restarting ? '重启中…' : '重启 daemon' }}
        </button>
      </footer>
    </article>
  </div>
</template>
```

注意：任务状态只在内存里（窗口常驻不销毁；webview 被手动刷新时任务视图会从零开始，daemon 侧任务仍会继续跑完——可接受的已知边界，spec §9 同级）。

- [ ] **Step 4: PluginsApp.vue 接入对话框与任务中心**

import 区追加：

```ts
import InstallDialog from './InstallDialog.vue'
import TaskCenter from './TaskCenter.vue'
```

`notice` 声明之后追加：

```ts
const dialog = ref<'install' | 'remove' | null>(null)

function onTaskSubmitted(): void {
  tab.value = 'tasks'
}
```

`<header class="bar">` 里 `<span class="daemon-state" …>` 之前加：

```html
      <div class="bar-actions">
        <button class="mini" @click="dialog = 'install'">安装插件</button>
        <button class="mini" @click="dialog = 'remove'">移除插件</button>
      </div>
```

panel 里 `<p v-else class="waiting">（后续切片交付此面板）</p>` 改为：

```html
        <TaskCenter v-else-if="tab === 'tasks'" @notice="showNotice" />
        <p v-else class="waiting">（后续切片交付此面板）</p>
```

`</main>` 之前加：

```html
    <InstallDialog v-if="dialog" :mode="dialog" @close="dialog = null" @submitted="onTaskSubmitted" />
```

- [ ] **Step 5: styles.css 追加对话框/任务卡样式**

```css
.dialog-mask {
  position: fixed;
  inset: 0;
  background: rgba(4, 8, 20, 0.6);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 10;
}

.dialog {
  width: min(460px, 90vw);
  background: var(--bg-soft);
  border: 1px solid var(--border);
  border-radius: 14px;
  padding: 20px;
}

.dialog h2 {
  margin: 0 0 8px;
  font-size: 16px;
}

.dialog input {
  width: 100%;
  background: rgba(11, 16, 32, 0.9);
  border: 1px solid var(--border);
  border-radius: 8px;
  color: var(--fg);
  padding: 8px 10px;
  font-size: 13px;
}

.warn {
  color: #f0b35e;
  font-size: 13px;
}

.dialog footer {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 14px;
}

.spinner {
  width: 12px;
  height: 12px;
  border: 2px solid var(--border);
  border-top-color: var(--accent);
  border-radius: 50%;
  animation: plugins-spin 0.8s linear infinite;
}

@keyframes plugins-spin {
  to {
    transform: rotate(360deg);
  }
}

.done-banner {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-top: 8px;
  font-size: 13px;
  color: #f0b35e;
}
```

- [ ] **Step 6: 验证（M3 验收）**

```bash
pnpm build
```

手动验收（同前启动命令）：插件窗口 → "安装插件" → 输入一个必然失败的 spec（如 `definitely-not-a-real-pkg-zz`）→ 任务页签出现任务卡 → 输出尾滚动 → 失败态显示 pnpm 诊断与退出码。再验 spec 校验：输入 `foo&calc` 应被本地拒绝、不产生任务。（成功路径需要真实插件包，验收时如手头有可用的再补一次真实安装 → done → 点"重启 daemon" → daemon 回到 running。）

- [ ] **Step 7: Commit**

```bash
git add src/plugins/ src/styles.css
git commit -m "feat: 安装/移除对话框 + 任务中心 + 重启引导 —— M3 完成"
```

---

### Task 12: 设置面板（只读视图：describe + 值/schema/secret 槽位）

**Files:**
- Modify: `src/plugins/plugins.ts`
- Create: `src/plugins/SettingsPanel.vue`
- Modify: `src/plugins/PluginsApp.vue`
- Modify: `src/styles.css`

**Interfaces:**
- Consumes: Task 2 的 `dsh_api_call`（`settings.describe`）。
- Produces: `SettingsNamespaceView` / `SettingsDescription` / `describeSettings()`（Task 13 叠加写操作）；`SettingsPanel` 的 `notice` emit（与 Task 7 `showNotice` 对齐）。

- [ ] **Step 1: plugins.ts 追加**

```ts
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
```

- [ ] **Step 2: 写 SettingsPanel.vue（只读版）**

```vue
<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { describeSettings } from './plugins'
import type { SettingsNamespaceView } from './plugins'

const namespaces = ref<SettingsNamespaceView[]>([])
const writable = ref(false)
const loadError = ref<string | null>(null)
const loading = ref(true)

async function reload(): Promise<void> {
  loading.value = true
  try {
    const desc = await describeSettings()
    namespaces.value = desc.namespaces
    writable.value = desc.writable
    loadError.value = null
  } catch (err) {
    loadError.value = String(err)
  } finally {
    loading.value = false
  }
}

onMounted(() => void reload())

function pretty(value: unknown): string {
  return JSON.stringify(value ?? {}, null, 2)
}
</script>

<template>
  <div class="settings">
    <p v-if="loading" class="waiting">正在读取设置命名空间…</p>
    <p v-else-if="loadError" class="error">{{ loadError }}</p>
    <template v-else>
      <p v-if="!writable" class="hint">当前设置为只读（上游声明 writable=false）。</p>
      <p v-if="!namespaces.length" class="waiting">没有设置命名空间。</p>
      <article v-for="ns in namespaces" :key="ns.ns" class="ns-card">
        <header>
          <code class="mono">{{ ns.ns }}</code>
          <span class="badge" :data-applies="ns.applies">
            {{ ns.applies === 'live' ? '保存即生效' : '重启后生效' }}
          </span>
        </header>
        <details>
          <summary>当前值（已脱敏）</summary>
          <pre class="json">{{ pretty(ns.value) }}</pre>
        </details>
        <details>
          <summary>schema</summary>
          <pre class="json">{{ pretty(ns.schema) }}</pre>
        </details>
        <div v-if="ns.secrets.length" class="secrets">
          <h3>密钥槽位</h3>
          <div v-for="slot in ns.secrets" :key="slot.path.join('.')" class="secret-row">
            <code class="mono">{{ slot.path.join('.') }}</code>
            <span :class="['badge', slot.set ? 'on' : 'off']">{{ slot.set ? '已配置' : '未配置' }}</span>
          </div>
        </div>
      </article>
    </template>
  </div>
</template>
```

- [ ] **Step 3: PluginsApp.vue 接入设置页签**

import 区追加：

```ts
import SettingsPanel from './SettingsPanel.vue'
```

panel 里 `<p v-else class="waiting">（后续切片交付此面板）</p>` 改为：

```html
        <SettingsPanel v-else-if="tab === 'settings'" />
        <p v-else class="waiting">（后续切片交付此面板）</p>
```

（`v-if` 挂载语义 = "进入该区时拉取"：切走即销毁，下次进入重新 describe；未保存的编辑随之丢弃，符合预期。）

- [ ] **Step 4: styles.css 追加设置面板样式**

```css
details summary {
  cursor: pointer;
  font-size: 12px;
  color: var(--fg-dim);
  margin: 6px 0;
}

.secrets h3 {
  font-size: 13px;
  margin: 12px 0 4px;
}

.secret-row {
  display: flex;
  align-items: center;
  gap: 8px;
  margin: 6px 0;
}

.secret-row input {
  flex: 1;
  background: rgba(11, 16, 32, 0.9);
  border: 1px solid var(--border);
  border-radius: 8px;
  color: var(--fg);
  padding: 5px 10px;
  font-size: 12px;
}
```

- [ ] **Step 5: 验证 + Commit**

```bash
pnpm build
```

手动：设置页签列出命名空间卡片（当前值/schema 可展开，secret 槽位显示已配置/未配置）。

```bash
git add src/plugins/ src/styles.css
git commit -m "feat: 设置面板只读视图 —— describe + 脱敏值/schema/secret 槽位"
```

---

### Task 13: 设置写入（JSON 编辑 + revision 冲突 + secret 增删）—— M4 完成

**Files:**
- Modify: `src/plugins/plugins.ts`
- Modify: `src/plugins/SettingsPanel.vue`（整个替换为可写版）

**Interfaces:**
- Consumes: Task 12 的全部。
- Produces: `updateSettings(ns, patch, expectedRevision?)`、`mutateSettings(ns, ops, expectedRevision?)`、`SettingsOp`；冲突文案约定：错误字符串以 `[settings-conflict]` 开头。

- [ ] **Step 1: plugins.ts 追加**

```ts
export const updateSettings = (ns: string, patch: unknown, expectedRevision?: number): Promise<unknown> =>
  apiCall('settings.update', { ns, patch, expectedRevision })

export type SettingsOp = { op: 'set'; path: string[]; value: unknown } | { op: 'unset'; path: string[] }

export const mutateSettings = (ns: string, ops: SettingsOp[], expectedRevision?: number): Promise<unknown> =>
  apiCall('settings.mutate', { ns, ops, expectedRevision })
```

- [ ] **Step 2: SettingsPanel.vue 整个替换为可写版**

```vue
<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { describeSettings, mutateSettings, updateSettings } from './plugins'
import type { SettingsNamespaceView } from './plugins'

const emit = defineEmits<{ notice: [text: string] }>()

const namespaces = ref<SettingsNamespaceView[]>([])
const writable = ref(false)
const loadError = ref<string | null>(null)
const loading = ref(true)

async function reload(): Promise<void> {
  loading.value = true
  try {
    const desc = await describeSettings()
    namespaces.value = desc.namespaces
    writable.value = desc.writable
    loadError.value = null
  } catch (err) {
    loadError.value = String(err)
  } finally {
    loading.value = false
  }
}

onMounted(() => void reload())

function pretty(value: unknown): string {
  return JSON.stringify(value ?? {}, null, 2)
}

// ---- user 层 JSON 编辑（每个 ns 一份编辑态，保存/冲突后重置）----

interface NsEdit {
  text: string
  error: string | null
  saving: boolean
}

const edits = ref<Record<string, NsEdit>>({})

function editOf(ns: SettingsNamespaceView): NsEdit {
  if (!edits.value[ns.ns]) {
    edits.value[ns.ns] = { text: pretty(ns.user ?? {}), error: null, saving: false }
  }
  return edits.value[ns.ns]
}

function isConflict(err: unknown): boolean {
  return String(err).startsWith('[settings-conflict]')
}

async function save(ns: SettingsNamespaceView): Promise<void> {
  const edit = editOf(ns)
  let patch: unknown
  try {
    patch = JSON.parse(edit.text || '{}')
  } catch {
    edit.error = 'JSON 解析失败，请检查语法'
    return
  }
  if (patch === null || typeof patch !== 'object' || Array.isArray(patch)) {
    edit.error = '补丁必须是 JSON 对象（merge 进 user 层）'
    return
  }
  edit.saving = true
  edit.error = null
  try {
    await updateSettings(ns.ns, patch, ns.revision)
    await reload()
    delete edits.value[ns.ns]
    emit(
      'notice',
      ns.applies === 'restart' ? `已保存；${ns.ns} 需重启 daemon 后生效` : `已保存，${ns.ns} 即时生效`,
    )
  } catch (err) {
    if (isConflict(err)) {
      await reload()
      delete edits.value[ns.ns]
      editOf(ns).error = '设置已被他处修改，已载入最新值，请核对后重新保存'
    } else {
      edit.error = String(err)
    }
  } finally {
    edit.saving = false
  }
}

// ---- secret 槽位（write-only，清除走 unset）----

const secretInputs = ref<Record<string, string>>({})
const secretBusy = ref<string | null>(null)
const secretErrors = ref<Record<string, string | null>>({})

function secretKey(ns: string, path: string[]): string {
  return `${ns}//${path.join('.')}`
}

async function setSecret(ns: SettingsNamespaceView, path: string[]): Promise<void> {
  const key = secretKey(ns.ns, path)
  const value = secretInputs.value[key] ?? ''
  if (!value) return
  secretBusy.value = key
  secretErrors.value[key] = null
  try {
    await mutateSettings(ns.ns, [{ op: 'set', path, value }], ns.revision)
    secretInputs.value[key] = ''
    await reload()
    emit('notice', `密钥 ${path.join('.')} 已写入（write-only，不回显）`)
  } catch (err) {
    if (isConflict(err)) {
      await reload()
      secretErrors.value[key] = '设置已被他处修改，已刷新，请重试'
    } else {
      secretErrors.value[key] = String(err)
    }
  } finally {
    secretBusy.value = null
  }
}

async function clearSecret(ns: SettingsNamespaceView, path: string[]): Promise<void> {
  const key = secretKey(ns.ns, path)
  secretBusy.value = key
  secretErrors.value[key] = null
  try {
    await mutateSettings(ns.ns, [{ op: 'unset', path }], ns.revision)
    await reload()
    emit('notice', `密钥 ${path.join('.')} 已清除`)
  } catch (err) {
    if (isConflict(err)) {
      await reload()
      secretErrors.value[key] = '设置已被他处修改，已刷新，请重试'
    } else {
      secretErrors.value[key] = String(err)
    }
  } finally {
    secretBusy.value = null
  }
}
</script>

<template>
  <div class="settings">
    <p v-if="loading" class="waiting">正在读取设置命名空间…</p>
    <p v-else-if="loadError" class="error">{{ loadError }}</p>
    <template v-else>
      <p v-if="!writable" class="hint">当前设置为只读（上游声明 writable=false）。</p>
      <p v-if="!namespaces.length" class="waiting">没有设置命名空间。</p>
      <article v-for="ns in namespaces" :key="ns.ns" class="ns-card">
        <header>
          <code class="mono">{{ ns.ns }}</code>
          <span class="badge" :data-applies="ns.applies">
            {{ ns.applies === 'live' ? '保存即生效' : '重启后生效' }}
          </span>
        </header>
        <details>
          <summary>当前值（已脱敏）</summary>
          <pre class="json">{{ pretty(ns.value) }}</pre>
        </details>
        <details>
          <summary>schema</summary>
          <pre class="json">{{ pretty(ns.schema) }}</pre>
        </details>

        <div class="editor">
          <label :for="`edit-${ns.ns}`">用户层补丁（JSON 对象，merge 进 user 层）</label>
          <textarea
            :id="`edit-${ns.ns}`"
            v-model="editOf(ns).text"
            rows="8"
            spellcheck="false"
            :disabled="!writable || editOf(ns).saving"
          ></textarea>
          <p v-if="editOf(ns).error" class="error">{{ editOf(ns).error }}</p>
          <button class="mini" :disabled="!writable || editOf(ns).saving" @click="save(ns)">
            {{ editOf(ns).saving ? '保存中…' : '保存' }}
          </button>
        </div>

        <div v-if="ns.secrets.length" class="secrets">
          <h3>密钥槽位</h3>
          <div v-for="slot in ns.secrets" :key="slot.path.join('.')">
            <div class="secret-row">
              <code class="mono">{{ slot.path.join('.') }}</code>
              <span :class="['badge', slot.set ? 'on' : 'off']">{{ slot.set ? '已配置' : '未配置' }}</span>
              <input
                v-model="secretInputs[secretKey(ns.ns, slot.path)]"
                type="password"
                placeholder="输入新值（write-only）"
                :disabled="!writable || secretBusy !== null"
              />
              <button
                class="mini"
                :disabled="!writable || secretBusy !== null || !secretInputs[secretKey(ns.ns, slot.path)]"
                @click="setSecret(ns, slot.path)"
              >
                写入
              </button>
              <button
                v-if="slot.set"
                class="mini danger"
                :disabled="!writable || secretBusy !== null"
                @click="clearSecret(ns, slot.path)"
              >
                清除
              </button>
            </div>
            <p v-if="secretErrors[secretKey(ns.ns, slot.path)]" class="error">
              {{ secretErrors[secretKey(ns.ns, slot.path)] }}
            </p>
          </div>
        </div>
      </article>
    </template>
  </div>
</template>
```

- [ ] **Step 3: PluginsApp.vue 把 `@notice` 接上**

`SettingsPanel` 标签改为：

```html
        <SettingsPanel v-else-if="tab === 'settings'" @notice="showNotice" />
```

- [ ] **Step 4: styles.css 追加编辑器样式**

```css
.editor label {
  display: block;
  font-size: 12px;
  color: var(--fg-dim);
  margin: 10px 0 4px;
}

.editor textarea {
  width: 100%;
  background: rgba(4, 8, 20, 0.7);
  border: 1px solid var(--border);
  border-radius: 8px;
  color: var(--fg);
  font-family: 'Cascadia Code', Consolas, monospace;
  font-size: 12px;
  padding: 8px 10px;
  resize: vertical;
}
```

- [ ] **Step 5: 验证（M4 验收）**

```bash
pnpm build
```

手动：设置页签任选一 ns → 编辑器里改一个无害键值（如加 `"note": "test"`）→ 保存 → notice 提示生效方式 → 当前值里出现该键（user 层 merge 成功）。非法 JSON 保存 → 行内错误。secret 槽位（若有）写入一个值 → 提示已写入、当前值里不回显。冲突路径需要两个写者，手动不易复现，代码路径以审读为准。

- [ ] **Step 6: Commit**

```bash
git add src/plugins/ src/styles.css
git commit -m "feat: 设置写入 —— user 层 JSON 编辑 + revision 冲突处理 + secret 增删 —— M4 完成"
```

---

### Task 14: 动态插件停止/删除 —— M5 主体

**Files:**
- Modify: `src/plugins/plugins.ts`
- Modify: `src/plugins/DynamicList.vue`（整个替换为可操作版）
- Modify: `src/plugins/PluginsApp.vue`
- Modify: `src/styles.css`

**Interfaces:**
- Consumes: Task 4 的 `DynamicPluginRow` 与轮询；Task 7 的 `showNotice`。
- Produces: `stopDynamicPlugin(agentId, pluginId)`、`undefineDynamicPlugin(agentId, pluginId)`；`DynamicStopReceipt` / `DynamicUndefineReceipt`。

- [ ] **Step 1: plugins.ts 追加**

```ts
// ---- 动态插件操作（回执是 value 而非信封错误，前端自行判 ok）----

export type DynamicStopReceipt = { ok: true } | { ok: false; reason: string; message?: string }

export const stopDynamicPlugin = (agentId: string, pluginId: string): Promise<DynamicStopReceipt> =>
  apiCall('dynamicCordisRunner/stopFromPanel', { agentId, pluginId })

export type DynamicUndefineReceipt =
  | { ok: true; wasRunning: boolean }
  | { ok: false; reason: string; message?: string }

export const undefineDynamicPlugin = (agentId: string, pluginId: string): Promise<DynamicUndefineReceipt> =>
  apiCall('dynamicCordisRunner/undefineFromPanel', { agentId, pluginId })
```

- [ ] **Step 2: DynamicList.vue 整个替换为可操作版**

```vue
<script setup lang="ts">
import { ref } from 'vue'
import { stopDynamicPlugin, undefineDynamicPlugin } from './plugins'
import type { DynamicPluginRow } from './plugins'

defineProps<{ rows: DynamicPluginRow[] }>()

const emit = defineEmits<{
  refresh: []
  notice: [text: string]
}>()

/** 一次只允许一个操作；删除用两段式按钮代替原生 confirm（webview 里不可靠）。 */
const acting = ref<string | null>(null)
const confirming = ref<string | null>(null)

/** 当前生效包（运行中的包优先，其次 last-success，再其次最新版本）。 */
function currentPackage(row: DynamicPluginRow) {
  const id = row.activeRun?.packageId ?? row.currentPackageId
  return row.packages.find((p) => p.packageId === id) ?? row.packages[row.packages.length - 1]
}

function halfLabel(status: string | undefined): string {
  switch (status) {
    case 'running':
      return '运行中'
    case 'waiting':
      return '等待依赖'
    case 'pending':
      return '加载中'
    case 'stopped':
      return '已停止'
    case 'failed':
      return '失败'
    case 'absent':
      return '无此半'
    default:
      return '—'
  }
}

function runStatusLabel(row: DynamicPluginRow): string {
  if (row.activeRun) return '运行中'
  switch (row.latestRun?.status) {
    case 'awaiting-approval':
      return '等待批准'
    case 'starting-host':
      return '启动中'
    case 'client-pending':
      return '等待页面'
    case 'waiting':
      return '等待依赖'
    case 'rejected':
      return '已拒绝'
    case 'failed':
      return '失败'
    case 'cancelled':
      return '已取消'
    case 'stopped':
      return '已停止'
    default:
      return '未运行'
  }
}

async function stop(row: DynamicPluginRow): Promise<void> {
  acting.value = row.pluginId
  try {
    const receipt = await stopDynamicPlugin(row.agentId, row.pluginId)
    emit('notice', receipt.ok ? '已停止' : `停止失败：${receipt.message ?? receipt.reason}`)
    emit('refresh')
  } catch (err) {
    emit('notice', `停止失败：${String(err)}`)
  } finally {
    acting.value = null
  }
}

async function undefine(row: DynamicPluginRow): Promise<void> {
  acting.value = row.pluginId
  confirming.value = null
  try {
    const receipt = await undefineDynamicPlugin(row.agentId, row.pluginId)
    emit(
      'notice',
      receipt.ok
        ? receipt.wasRunning
          ? '已删除（其运行实例已一并停止）'
          : '已删除'
        : `删除失败：${receipt.message ?? receipt.reason}`,
    )
    emit('refresh')
  } catch (err) {
    emit('notice', `删除失败：${String(err)}`)
  } finally {
    acting.value = null
  }
}
</script>

<template>
  <div class="dynamic">
    <p class="hint">动态插件由模型在会话中定义，会话级、进程内，daemon 重启即失。</p>
    <p v-if="!rows.length" class="waiting">当前没有动态插件。</p>
    <article v-for="row in rows" :key="row.pluginId" class="dyn-card">
      <header>
        <strong>{{ currentPackage(row)?.name || row.pluginId }}</strong>
        <span class="phase" :data-phase="row.activeRun ? 'active' : 'none'">{{ runStatusLabel(row) }}</span>
      </header>
      <p v-if="currentPackage(row)?.purpose" class="dim">{{ currentPackage(row)?.purpose }}</p>
      <dl class="kv">
        <dt>pluginId</dt>
        <dd class="mono">{{ row.pluginId }}</dd>
        <dt>所属会话</dt>
        <dd class="mono">{{ row.agentId }}</dd>
        <dt>包版本数</dt>
        <dd>{{ row.packages.length }}</dd>
        <dt>host 半</dt>
        <dd>
          {{ halfLabel(row.latestRun?.host.status) }}
          <template v-if="row.latestRun?.host.waitingFor?.length">
            （等待：{{ row.latestRun.host.waitingFor.join(', ') }}）
          </template>
        </dd>
        <dt>client 半</dt>
        <dd>{{ halfLabel(row.latestRun?.client.status) }}</dd>
      </dl>
      <pre v-if="row.latestRun?.error" class="error-detail">{{ row.latestRun.error.phase }}: {{ row.latestRun.error.message }}</pre>
      <footer class="actions">
        <button
          v-if="row.activeRun"
          class="mini"
          :disabled="acting !== null"
          @click="stop(row)"
        >
          {{ acting === row.pluginId ? '操作中…' : '停止' }}
        </button>
        <template v-if="confirming !== row.pluginId">
          <button class="mini danger" :disabled="acting !== null" @click="confirming = row.pluginId">删除</button>
        </template>
        <template v-else>
          <button class="mini danger" :disabled="acting !== null" @click="undefine(row)">确认删除（不可恢复）</button>
          <button class="mini" @click="confirming = null">取消</button>
        </template>
      </footer>
    </article>
  </div>
</template>
```

- [ ] **Step 3: PluginsApp.vue 接上 refresh/notice**

`DynamicList` 标签改为：

```html
        <DynamicList v-else-if="tab === 'dynamic'" :rows="dynamicRows" @refresh="loadInventories" @notice="showNotice" />
```

- [ ] **Step 4: styles.css 追加**

```css
.actions {
  display: flex;
  gap: 8px;
  margin-top: 8px;
}
```

- [ ] **Step 5: 验证 + Commit**

```bash
pnpm build
```

手动：动态插件页签（有动态插件时）→ 停止 → 卡片状态翻转为已停止 → 删除两段确认 → 卡片消失。没有动态插件时验空态即可（真实动态插件需要一次 agent 会话现场定义，验收时视情况）。

```bash
git add src/plugins/ src/styles.css
git commit -m "feat: 动态插件停止/删除（两段式确认）"
```

---

### Task 15: architecture.md 补节 + 全量收尾验证 —— M5 完成

**Files:**
- Modify: `docs/architecture.md`

**Interfaces:**
- Consumes: Task 1–14 全部落地结果。
- Produces: 文档一节，说明插件管理是首个 syscall 表扩展实例。

- [ ] **Step 1: 读 docs/architecture.md，追加一节**

在文末追加（小标题层级与全文一致；下为内容要点，成文时与全文文风对齐）：

- 插件管理 = syscall 表的第一个扩展实例：`dsh_api_call`（通用 RPC 透传）、`plugin_set_enabled`（patch 托管行）、`plugin_install/remove`（CLI 任务队列）、`open_plugins_window`；事件 `plugins://task`。
- `dsh-bridge` crate：daemon `/api` 信封的客户端半；webview 过不了上游信任栅栏，一切 API 经 Rust loopback 转发——机制层，策略留在 dsh。
- `dsh-profile` crate：`cordis.patch.yml` 文本级托管行（绝不 YAML 解析，`!!js` 安全）+ `dsh plugin` 串行任务队列（pnpm profile 锁）。
- 窗口拓扑：`main`（splash → 上游 Web UI）与 `plugins`（管理面，常驻隐藏），按窗口 label 选根组件，无 router。
- 已知边界指向 spec §9。

- [ ] **Step 2: 全量收尾验证**

```bash
cd src-tauri && cargo test
pnpm build
```

然后完整过一遍手动验收清单（M1–M5 各任务的验收路径，用全局约束里的启动命令）：

1. 入口按钮 → 插件窗口 → 清单/动态两页签有数据或空态
2. 开关翻转 → patch 文件出现托管行 → 轮询确认生效；备份文件存在
3. 安装一个必然失败的 spec → 任务卡失败态 + pnpm 诊断；非法 spec 本地拒绝
4. 设置 ns JSON 编辑保存 → 值更新；非法 JSON 行内报错
5. 动态插件停止/删除（有条件时）

- [ ] **Step 3: Commit**

```bash
git add docs/architecture.md
git commit -m "docs: architecture.md 补插件管理一节（首个 syscall 表扩展实例）—— M5 完成"
```

---

## 自审记录（写完计划后对照 spec 核对）

- **spec 覆盖**：§3.1 → Task 1/2 ✓；§3.2-1 → Task 5 ✓；§3.2-2 → Task 6 ✓；§3.2-3 → Task 8/9 ✓；§3.3 五命令+事件 → Task 2/3/6/10 ✓；§3.4 → Task 3/4 ✓；§3.5 四个页面行为 → Task 4/7/11/12/13/14 ✓；§4 错误处理 → 各任务错误字符串 + Task 13 冲突文案 ✓；§5 安全 → Task 1 method 白名单 / Task 6 备份 / Task 9 spec 校验 / Task 11 风险提示 / Task 13 write-only ✓；§6 测试策略 → crate 单测 + ignore 集成测试 + vue-tsc/build + 手动验收 ✓；§7 触碰点 → 全部出现在任务 Files 里 ✓；§8 切片 → Task 4=M1、7=M2、11=M3、13=M4、14/15=M5 ✓。
- **占位符**：无 TBD/TODO；所有代码步骤含完整代码。
- **类型一致性**：`PluginTaskView`（Rust camelCase）↔ TS 镜像字段逐一对齐；`PendingIntent`/`showNotice`/`loadInventories` 跨任务引用一致；`setPluginEnabled(entryId, enabled)` 内部取反为 `disabled`，与 Rust 命令参数一致。
- **自审修正记录**：Task 9 初稿的 stdout/stderr 读取闭包无法对两种流类型泛化，已改为泛型函数 `spawn_reader` 直接内联进实现（不保留"先错后改"两步）。
