# 窗口、全局快捷键与托盘

桌面集成的行为约束：哪些是 OS / WebView 层处理的、哪些是 Rust 层显式实现的，以及为什么这么做。

---

## 窗口拓扑

`tauri.conf.json` 声明三个 WebView 窗口：

| label | 标题 | 可见 | 特殊 |
|---|---|---|---|
| `main` | DeepSeek Harness | ✅ | 启动期渲染 splash → 就绪后导航到 `http://127.0.0.1:<port>` |
| `plugins` | 插件管理 | ❌ 默认隐藏 | 从托盘 / 命令面板 / `pluginInventory/list` 入口唤起 |
| `palette` | 命令面板 | ❌ 默认隐藏 | `decorations: false`、`transparent: true`、`alwaysOnTop: true`、`skipTaskbar: true` |

入口 (`src/main.ts`) 按 `window.__TAURI_INTERNALS__.metadata.currentWindow.label` 选择根组件（`App.vue` / `PluginsApp.vue` / `PaletteApp.vue`），无路由。

---

## 全局快捷键

`Ctrl+Shift+P` 注册为命令面板的全局快捷键（`tauri-plugin-global-shortcut`）。

为什么必须在 Rust / OS 层注册，而不是 WebView `keydown` 监听：

- 主窗口就绪后导航到 `http://127.0.0.1:<port>`（上游 dsh Web UI，同源 WebView）；
- Windows / WebView2 在 host 层把 `Ctrl+Shift+P` 绑为打印预览快捷键；
- `keydown` 事件被 host 层截走，JS 端永远收不到，`preventDefault` 无效。

`tauri-plugin-global-shortcut` 在 Windows 用 `RegisterHotKey`，在 OS 层捕获按键后才转发到 Rust，不被 WebView2 截走。

注册代码位于 `src-tauri/src/lib.rs::install_palette_global_shortcut`：

```rust
let shortcut = Shortcut::new(
    Some(Modifiers::CONTROL | Modifiers::SHIFT),
    Code::KeyP,
);
app.global_shortcut().on_shortcut(shortcut, move |_app, _sc, event| {
    if event.state == ShortcutState::Pressed {
        open_palette(&handle);
    }
})?;
```

---

## 命令面板（`palette` 窗口）

`palette` 窗口是 `transparent: true` + `decorations: false` 的浮层。每次打开都重新居中在 `main` 上：

```rust
fn center_over(target: &WebviewWindow, overlay: &WebviewWindow) {
    let target_pos  = target.outer_position()?;
    let target_size = target.outer_size()?;
    let overlay_size = overlay.outer_size()?;
    let x = target_pos.x + (target_size.width as i32 - overlay_size.width as i32) / 2;
    let y = target_pos.y + (target_size.height as i32 - overlay_size.height as i32) / 2;
    overlay.set_position(PhysicalPosition::new(x, y));
}
```

为什么每次重算：Tauri `center: true` 配置只在窗口创建时生效一次；`hide()` 复用窗口后位置漂移。每次 `open_palette` 先 `center_over(main, palette)`。

CSS 上必须把 `body` 背景设为透明、卡片用实心背景 —— 否则会泄漏共享 CSS 的 body 渐变（详见 UI 偏好备注）。

### 焦点 / z-order 抖动的兜底

`open_plugins_window` / `show_main_window` 在切换目标窗口前先 `hide()` 命令面板 —— WebView 端 `await close()` 在 Windows 上不能保证透明无边框窗口的 z-order 残留，残留会导致 `set_focus` 重新激活那个看不见的 palette。

---

## 关闭主窗口 ≠ 退出应用

```rust
app.run(|app_handle, event| {
    if let tauri::RunEvent::ExitRequested { code, api, .. } = event {
        if code.is_none() {
            api.prevent_exit();
            if let Some(window) = app_handle.get_webview_window("main") {
                let _ = window.hide();   // 最小化到托盘
            }
        } else {
            // 显式退出（托盘 quit / app_quit）：走干净停机
            if let Some(state) = app_handle.try_state::<DaemonState>() {
                state.supervisor.stop();
            }
        }
    }
});
```

- `code.is_none()` 表示窗口关闭按钮触发，**阻止退出**并 `hide()` 主窗口，进程继续在托盘运行。
- 显式退出（托盘「退出」菜单 / 命令面板 `quit`）走 `app.exit(0)`，触发 `ExitRequested { code: Some(0) }`，supervisor 干净停机后进程终止。

---

## 系统托盘

注册在 `install_tray`。菜单项 ID 与命令名一一对应：

| 菜单 ID | 行为 | 走的 IPC |
|---|---|---|
| `show_palette` | 调起命令面板 | `open_palette`（center + show + focus + emit `palette://open`） |
| `show_plugins` | 显示并聚焦 `plugins` 窗口 | `show_window("plugins")` |
| `show_main` | 显示并聚焦 `main` 窗口 | `show_window("main")` |
| `restart` | 重启 dsh 守护进程 | `supervisor.restart()` |
| `quit` | 退出应用 | `app.exit(0)` |

左键单击托盘图标 → 直接 `open_palette`（`show_menu_on_left_click(false)`，避免默认行为吞掉左键）。

图标复用 `app.default_window_icon()`，无需额外资产。

---

## 窗口能力清单

`src-tauri/capabilities/default.json`：

```json
{
  "identifier": "default",
  "description": "主窗口、插件窗口、命令面板窗口的基础能力：事件与窗口 API。",
  "windows": ["main", "plugins", "palette"],
  "permissions": ["core:default"]
}
```

三窗口共用同一能力集——`core:default` 只含事件与窗口 API，最小化权限面。窗口级 capability 收紧可在此文件按 `windows` 字段单独控制。

---

## CSP

`tauri.conf.json`：

```
default-src 'self';
connect-src 'self' ipc: http://ipc.localhost http://127.0.0.1:* ws://127.0.0.1:*;
style-src 'self' 'unsafe-inline';
img-src 'self' data:
```

- `connect-src` 包括 `ipc:` + `http://ipc.localhost`（Tauri 2 IPC）+ `http://127.0.0.1:*` / `ws://127.0.0.1:*`（守护进程 loopback）。
- `style-src 'unsafe-inline'` 仅为支持 Vue scoped styles 与运行时主题变量；不引入外部样式表。