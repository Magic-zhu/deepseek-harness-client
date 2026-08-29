# DeepSeek Harness Client

DeepSeek Harness（`dsh`）的 Tauri 2 桌面客户端。`dsh` 是面向 dsh 生态的本地 AI 编程代理与插件宿主，本仓库提供一个开箱即用的桌面外壳：拉起 `dsh web` 守护进程、托管插件安装/卸载与配置、并提供命令面板、系统托盘等桌面集成。

设计取向遵循 Unix 哲学：客户端只做「监督 + 装载」，不改 `dsh` 内核、不自造协议。完整设计见 [docs/architecture.md](docs/architecture.md)。

---

## 主要特性

- **零配置启动**：检测到 Node ≥ 22.19（或 ≥ 24）后自动拉起 `dsh web` 守护进程；就绪后窗口直接加载上游 Web UI。
- **进程监督**：守护进程崩溃时按指数退避（500 ms · 2ⁿ，上限 30 s）自动重启；手动「立即重试」可重置退避。
- **级联终止**：Windows Job Object（kill-on-close）/ Unix 进程组，应用退出时整棵守护进程树一并终止。
- **命令面板**：`Ctrl+Shift+P` 或托盘图标，全局调起命令面板窗口（覆盖在主窗口之上）。
- **插件管理**：图形化的安装/卸载/启用/禁用界面，底层走 `dsh plugin` CLI 串行任务队列。
- **系统托盘**：右键菜单可快速调起命令面板、打开插件管理、重启守护进程或退出应用；关闭窗口最小化到托盘。

## 环境要求

| 依赖 | 版本 | 说明 |
|---|---|---|
| Node.js | ≥ 22.19 或 ≥ 24 | `dsh` 上游 `engines.node` 硬性要求 |
| pnpm | ≥ 9 | 仅构建本客户端需要 |
| Rust | ≥ 1.77（stable，含 rustfmt、clippy） | `rust-toolchain.toml` 自动固定 |
| 操作系统 | Windows 10+ / macOS 11+ / Linux（Ubuntu LTS） | 三平台 CI 覆盖 |

> 工具链版本固定在 [`.nvmrc`](.nvmrc)（Node 22）与 [`rust-toolchain.toml`](rust-toolchain.toml)。

### Node 版本前置检查

客户端启动时会校验实际将运行 `dsh` 的 Node 版本。若不达标，窗口停留在引导页：

- 列出当前 Node 版本、PATH 上的 `node` 路径、所需版本；
- 给出 `nvm` 等版本管理器的切换步骤；
- 显示计划启动的命令，但**不拉起守护进程**。

切换好 Node 后需要**完全退出并重新启动本应用**——仅点击「重新检测」不会启动守护进程。

## 快速开始

```sh
# 1. 安装依赖
pnpm install

# 2. （首次）生成图标：src-assets/icon.png → src-tauri/icons/*
pnpm icon

# 3. 开发模式
pnpm tauri dev

# 4. 产出 NSIS 安装包（Windows）
pnpm tauri build
```

### 守护进程启动命令解析顺序

按顺序取第一个可用的命令：

1. 环境变量 `DSH_CLIENT_BIN`（自定义命令，可带前缀参数，例如 `node H:\code\deepseek-harness\apps\cli\lib\bin.js`）
2. PATH 上的 `dsh`
3. PATH 上的 `npx -y @deepseek-ai/dsh@latest`

最终统一追加参数 `web --port 0`（由 OS 选择空闲端口）。就绪判定依赖上游契约：stdout 出现 `dsh web: http://127.0.0.1:<port>` 即视为 `/api` 已挂载。

> **已知限制**：`DSH_CLIENT_BIN` 含空格路径时经 `cmd /c` 的引号处理有限，建议优先使用 PATH 上的 `dsh`。

## 持续集成

GitHub Actions（[`.github/workflows/ci.yml`](.github/workflows/ci.yml)）：

- **lint-build**：PR / push 到 `main` 时三平台（ubuntu / windows / macos）执行
  - `pnpm build`（`vue-tsc --noEmit` + `vite build`）
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace`
- **package-windows**：推送 `v*` tag 时 Windows runner 跑 `pnpm tauri build`，自动把 NSIS 安装包作为 release asset 上传。

## 项目结构

```
deepseek-harness-client/
├── src/                          # Vue 3 客户端平面（启动页、插件窗口、命令面板）
│   ├── App.vue                   # 主窗口（splash → 上游 Web UI）
│   ├── plugins/                  # 插件管理窗口（plugins window）
│   ├── palette/                  # 命令面板窗口
│   ├── daemon.ts                 # 守护进程状态/事件连接器
│   ├── palette.ts                # 命令面板 IPC 客户端
│   ├── main.ts                   # 入口，按窗口 label 选根组件
│   └── styles.css
├── src-tauri/
│   ├── crates/
│   │   ├── dsh-supervisor/       # 进程监督：spawn / 就绪探测 / 退避 / Job Object
│   │   ├── dsh-bridge/           # /api 信封的客户端半（loopback HTTP 中继）
│   │   └── dsh-profile/          # cordis.patch.yml 托管行 + 插件 CLI 串行任务队列
│   ├── src/
│   │   ├── lib.rs                # 薄命令层 + 事件转发 + 托盘 + 全局快捷键
│   │   └── main.rs
│   ├── capabilities/default.json # Tauri 能力清单
│   ├── icons/                    # 图标资源
│   ├── tauri.conf.json           # 窗口、bundle、CSP
│   └── Cargo.toml                # workspace 元数据
├── docs/
│   ├── architecture.md           # 架构设计（三平面、IPC 契约、生命周期）
│   ├── ipc.md                    # Tauri 命令与事件参考
│   ├── plugin-management.md      # 插件管理子系统
│   └── windows-shortcuts.md      # 窗口、全局快捷键、托盘
├── scripts/make-icon.mjs         # 零依赖图标源图生成
├── .github/workflows/ci.yml
├── .nvmrc
├── rust-toolchain.toml
├── package.json
├── tsconfig.json
└── vite.config.ts
```

`dsh-supervisor` / `dsh-bridge` / `dsh-profile` 三个 crate 依赖方向单向，不引用 Tauri，可独立测试。

## 安全模型

- 一切 RPC 经 loopback HTTP。客户端**不绑定、不转发任何非本机地址**，与 `dsh` 的信任栅栏一致。
- `daemon_api_call` 桥接所有 `/api` 调用：WebView 无法满足 loopback Host / Origin 校验，故一切经 Rust 转发。
- Tauri capability 最小化（`core:default`），仅启用事件与窗口 API。
- CSP：`default-src 'self'`；`connect-src` 仅允许 `'self' ipc: http://ipc.localhost http://127.0.0.1:* ws://127.0.0.1:*`；`style-src 'self' 'unsafe-inline'`；`img-src 'self' data:`。
- 凭据永不下发到 WebView。

## 故障排查

| 现象 | 排查 |
|---|---|
| 窗口停留在「引导页」 | 切换 Node 版本（≥ 22.19 或 ≥ 24），完全退出应用后重启 |
| 守护进程反复崩溃 | 查看启动页「最近日志」；启用 `DSH_CLIENT_LOG=dsh_client_lib=debug,dsh_supervisor=info` 复现 |
| 命令面板按 `Ctrl+Shift+P` 没反应 | 该快捷键由 Tauri 全局快捷键插件注册（`RegisterHotKey`），不会被 WebView2 截获；若仍无效，检查是否被其他工具占用 |
| 关闭主窗口后进程仍在 | 设计如此：关闭窗口会最小化到托盘；从托盘菜单「退出」才彻底退出 |
| 端口被占用 | `--port 0` 由 OS 选空闲端口；不应发生。如发生，重启 `dsh web` 即可 |

## 贡献

1. Fork & 创建特性分支
2. `pnpm install` + `pnpm tauri dev` 验证本地可运行
3. 提交前确保 CI 全绿：`pnpm build`、`cargo fmt`、`cargo clippy -- -D warnings`、`cargo test`
4. 提交 PR 时附上对设计/契约的影响说明

更多开发约定、IPC 契约、插件子系统设计见 `docs/` 目录。

## 许可证

[MIT](LICENSE)