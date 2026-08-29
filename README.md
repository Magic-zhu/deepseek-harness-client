# deepseek-harness-client

DeepSeek Harness（`dsh`）的 Tauri 桌面客户端。P0 目标：**双击即用** —— 应用自动拉起未修改的 `dsh web` 守护进程（sidecar），就绪后窗口进入上游 Web UI；守护进程崩溃则自动退避重启，并把用户带回启动页。

架构遵循 Unix 设计哲学：客户端只做"监督 + 装载"，不改内核、不自造协议。详见 [docs/architecture.md](docs/architecture.md)。

## 前置条件

- **Node.js ≥ 22.19（或 ≥ 24）**——dsh 上游 `engines` 硬性要求；Node 20 的旧 npm（≤9.x）在 Windows 上安装 dsh 依赖树时会大面积 EPERM 失败。多版本机器可用 nvm 切换：`nvm use 24.x`。
- pnpm ≥ 9（仅构建本客户端需要）
- Rust ≥ 1.77（`cargo --version` 确认）

> 客户端启动时会自动校验实际将跑 dsh 的 Node 版本；若不达标会显示引导页（列出当前版本、PATH 上的 node 路径、`nvm` 切换步骤、计划启动的命令），并**不会**拉起 `dsh` 守护进程。切换好 Node 后**完全退出并重新启动本应用**——仅点击"重新检测"不会启动守护进程。

## 快速开始

```sh
pnpm install
pnpm icon        # 首次：生成图标（src-assets/icon.png → src-tauri/icons/*）
pnpm tauri dev   # 开发模式
pnpm tauri build # 产出 NSIS 安装包
```

## CI

- PR / push 到 `main`：三平台（ubuntu / windows / macos）跑 `pnpm build` + `cargo fmt --check` + `cargo clippy --workspace --all-targets -- -D warnings` + `cargo test --workspace`。
- 推送 `v*` tag：Windows runner 跑 `pnpm tauri build`，自动把 NSIS 安装包作为 release asset 上传。

工具链版本固定在 `.nvmrc`（Node 22）和 `rust-toolchain.toml`（stable，带 rustfmt / clippy）。

## 守护进程启动命令解析

按顺序取第一个可用的：

1. 环境变量 `DSH_CLIENT_BIN`（自定义命令，可带前缀参数，如 `node H:\code\deepseek-harness\apps\cli\lib\bin.js`）
2. PATH 上的 `dsh`
3. PATH 上的 `npx -y @deepseek-ai/dsh@latest`

最终统一追加参数 `web --port 0`（由 OS 选择空闲端口）。就绪判定依据上游契约：stdout 出现 `dsh web: http://127.0.0.1:<port>` 即表示 `/api` 已挂载（上游源码注释明说该行是"supervisors 的就绪信号"）。

## 进程与安全模型

- 监督：指数退避重启（500ms · 2ⁿ，上限 30s），手动"立即重试"会重置退避。
- 级联终止：Windows Job Object（kill-on-close）/ Unix 进程组；应用退出时整棵守护进程树一并终止。
- 事件面：`daemon://starting|ready|crashed|stopped|log`，命令面：`daemon_status` / `daemon_log_tail` / `daemon_restart` / `daemon_stop`。
- 一切经 loopback：客户端不绑定、不转发任何非本机地址，与 dsh 的信任栅栏一致。

## 目录

```
src/                     # 客户端平面（Vue 3 启动页；P2 起长出自有 UI）
src-tauri/
├── crates/dsh-supervisor/  # 进程监督 crate（可独立复用/测试）
└── src/                    # Tauri 装配：薄命令层 + 事件转发 + 导航
scripts/make-icon.mjs    # 零依赖图标源图生成
docs/architecture.md     # 架构设计
```
