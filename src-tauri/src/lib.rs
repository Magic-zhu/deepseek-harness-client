//! dsh-client application layer: a thin command surface plus event
//! forwarding. Mechanism only — every policy decision stays in dsh.

use dsh_profile::tasks::{PluginTaskKind, PluginTaskRunner};
use dsh_supervisor::{self, LogStream, RestartPolicy, Supervisor, SupervisorEvent};
use serde::Serialize;
use tauri::menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, Url, WebviewWindow};
use tracing::{debug, error, info, warn};

/// Managed application state: the supervisor handle.
#[derive(Clone)]
struct DaemonState {
    supervisor: Supervisor,
}

/// Plugin install/remove task queue. Independent of preflight: the CLI talks
/// to the profile directory, not the daemon.
#[derive(Clone)]
struct PluginTasksState {
    runner: PluginTaskRunner,
}

/// Persistent across both preflight-failed and preflight-passed lifecycles so
/// the `preflight_check` command can re-probe without re-resolving.
#[derive(Clone)]
struct PreflightState {
    launch: dsh_supervisor::LaunchPlan,
    bin_override: Option<String>,
    path_node: Option<std::path::PathBuf>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DaemonStatusDto {
    state: dsh_supervisor::State,
    pid: Option<u32>,
    port: Option<u16>,
    attempt: u32,
    restarts: u32,
    last_error: Option<String>,
    command: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct LogLineDto {
    stream: LogStream,
    line: String,
}

#[tauri::command]
async fn daemon_status(state: tauri::State<'_, DaemonState>) -> Result<DaemonStatusDto, String> {
    let status = state.supervisor.status().await;
    Ok(DaemonStatusDto {
        state: status.state,
        pid: status.pid,
        port: status.port,
        attempt: status.attempt,
        restarts: status.restarts,
        last_error: status.last_error,
        command: state.supervisor.command_display(),
    })
}

#[tauri::command]
async fn daemon_log_tail(
    state: tauri::State<'_, DaemonState>,
    max: Option<usize>,
) -> Result<Vec<LogLineDto>, String> {
    let max = max.unwrap_or(60).clamp(1, 500);
    Ok(state
        .supervisor
        .log_tail(max)
        .await
        .into_iter()
        .map(|(stream, line)| LogLineDto { stream, line })
        .collect())
}

#[tauri::command]
fn daemon_restart(state: tauri::State<DaemonState>) -> Result<(), String> {
    state.supervisor.restart();
    Ok(())
}

#[tauri::command]
fn daemon_stop(state: tauri::State<DaemonState>) -> Result<(), String> {
    state.supervisor.stop();
    Ok(())
}

#[tauri::command]
fn plugin_install(state: tauri::State<'_, PluginTasksState>, spec: String) -> Result<String, String> {
    state.runner.submit(PluginTaskKind::Install, spec)
}

#[tauri::command]
fn plugin_remove(state: tauri::State<'_, PluginTasksState>, spec: String) -> Result<String, String> {
    state.runner.submit(PluginTaskKind::Remove, spec)
}

/// Snapshot of queued/finished plugin tasks so a (re)mounted task center can
/// catch up without waiting for the next state-change event.
#[tauri::command]
fn plugin_tasks_list(state: tauri::State<'_, PluginTasksState>) -> Result<Vec<dsh_profile::tasks::PluginTaskView>, String> {
    Ok(state.runner.list())
}

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

/// Show and focus the main window. Mirrors `open_plugins_window` so the
/// command palette has a uniform "open X" surface.
#[tauri::command]
fn show_main_window(app: tauri::AppHandle) -> Result<(), String> {
    show_window(&app, "main")
}

/// Quit the whole application (used by the command palette). Goes through
/// `app.exit(0)` so the `RunEvent::ExitRequested` path still runs and the
/// supervisor is stopped cleanly.
#[tauri::command]
fn app_quit(app: tauri::AppHandle) {
    app.exit(0);
}

/// Flat wrapper so the UI gets one struct with both the probe result and
/// the resolved command string for the diagnostics card.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct PreflightReportDto {
    #[serde(flatten)]
    report: dsh_supervisor::PreflightReport,
    dsh_command_display: String,
}

#[tauri::command]
async fn preflight_check(state: tauri::State<'_, PreflightState>) -> Result<PreflightReportDto, String> {
    let report = dsh_supervisor::run_probe(
        state.bin_override.as_deref(),
        state.path_node.as_deref(),
        &state.launch.program,
    )
    .await;
    Ok(PreflightReportDto {
        dsh_command_display: state.launch.display(),
        report,
    })
}

/// Generic daemon /api pass-through. The webview can never satisfy the
/// daemon's trust fence (Origin/Sec-Fetch-Site), so every API call crosses
/// this bridge. `Err` strings: `ApiError` Display verbatim (`[code] message`
/// for business errors — the settings UI pattern-matches the prefix).
#[tauri::command]
async fn dsh_api_call(
    app: tauri::AppHandle,
    method: String,
    payload: serde_json::Value,
) -> Result<serde_json::Value, String> {
    // `Option<State>` is not a valid command arg in Tauri 2 (its Option impl
    // is for optional frontend args and requires Deserialize), so take the
    // AppHandle and probe for managed state: preflight failure never manages
    // `DaemonState`, and this yields a readable fast-fail instead of Tauri's
    // "state not managed".
    let Some(state) = app.try_state::<DaemonState>() else {
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

/// Write one managed patch line pair (`disabled: true|false`) for the entry.
/// Hot application is upstream's HMR; the frontend verifies by polling.
#[tauri::command]
fn plugin_set_enabled(entry_id: String, disabled: bool) -> Result<(), String> {
    let home = dsh_profile::home::resolve_dsh_home()
        .ok_or_else(|| "无法定位 dsh home（DSH_HOME 未设置且无法解析用户目录）".to_string())?;
    let patch = dsh_profile::home::patch_file(&dsh_profile::home::profile_dir(&home));
    dsh_profile::patch::set_disabled(&patch, &entry_id, disabled).map_err(|err| err.to_string())
}

/// The `daemon://` channel for each event variant.
fn channel_of(event: &SupervisorEvent) -> &'static str {
    match event {
        SupervisorEvent::Starting { .. } => "daemon://starting",
        SupervisorEvent::Ready { .. } => "daemon://ready",
        SupervisorEvent::Crashed { .. } => "daemon://crashed",
        SupervisorEvent::Stopped => "daemon://stopped",
        SupervisorEvent::Log { .. } => "daemon://log",
    }
}

fn navigate(window: &WebviewWindow, url: &Url) {
    if let Err(err) = window.navigate(url.clone()) {
        warn!(?url, %err, "navigate failed; falling back to location.replace");
        let Ok(literal) = serde_json::to_string(url.as_str()) else { return };
        let _ = window.eval(&format!("window.location.replace({literal})"));
    }
}

/// Show, un-minimize, and focus the window with the given label. Returns an
/// error if the window is not declared in tauri.conf.json.
fn show_window(app: &AppHandle, label: &str) -> Result<(), String> {
    let window = app
        .get_webview_window(label)
        .ok_or_else(|| format!("窗口 {label} 未创建"))?;
    let _ = window.unminimize();
    window.show().map_err(|e| e.to_string())?;
    window.set_focus().map_err(|e| e.to_string())?;
    Ok(())
}

/// Open the command palette window and ping it to reset/clear state.
/// Used by the tray-icon click and the global shortcut.
fn open_palette(app: &AppHandle) {
    if show_window(app, "palette").is_ok() {
        let _ = app.emit("palette://open", ());
    }
}

async fn forward_task(app: AppHandle, supervisor: Supervisor, home: Url) {
    let mut receiver = supervisor.subscribe();
    debug!("forward_task subscribed; entering recv loop");
    loop {
        match receiver.recv().await {
            Ok(event) => {
                debug!(channel = channel_of(&event), "forward_task recv");
                let _ = app.emit(channel_of(&event), &event);
                match &event {
                    SupervisorEvent::Ready { port, .. } => {
                        info!(port, "supervisor ready; navigating webview");
                        if let Ok(url) = Url::parse(&format!("http://127.0.0.1:{port}")) {
                            if let Some(window) = app.get_webview_window("main") {
                                navigate(&window, &url);
                            } else {
                                error!("supervisor Ready but main window is missing");
                            }
                        }
                    }
                    SupervisorEvent::Crashed { .. } | SupervisorEvent::Stopped => {
                        // Only pull the user back to the splash when the
                        // webview is actually sitting on the daemon page.
                        if let Some(window) = app.get_webview_window("main") {
                            if let Ok(current) = window.url() {
                                if current.origin() != home.origin() {
                                    navigate(&window, &home);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            Err(broadcast_error @ tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                warn!(%broadcast_error, "forward_task event lag");
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        }
    }
}

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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_tracing();

    let app = tauri::Builder::default()
        .on_page_load(|webview, _payload| {
            // 全局 page-load 钩子：过滤到主窗口，重新注入快捷键监听器。
            // 注入脚本自身通过 `__dsh_palette_listener_installed` 做了幂等，
            // 所以每次 page-load 都调用 install_palette_shortcut 也是安全的。
            if webview.label() == "main" {
                install_palette_shortcut_on_webview(webview);
            }
        })
        .setup(|app| {
            let bin_override = std::env::var("DSH_CLIENT_BIN").ok();
            let launch = dsh_supervisor::resolve_launch(bin_override.as_deref());
            let path_node = dsh_supervisor::find_first_node_in_path();
            let override_token = dsh_supervisor::extract_bin_token(bin_override.as_deref());

            app.manage(PreflightState {
                launch: launch.clone(),
                bin_override: override_token,
                path_node: path_node.clone(),
            });

            let invocation = dsh_supervisor::resolve_invocation(bin_override.as_deref());
            let runner = tauri::async_runtime::block_on(async { PluginTaskRunner::start(invocation) });
            app.manage(PluginTasksState { runner: runner.clone() });
            tauri::async_runtime::spawn(plugins_forward_task(app.handle().clone(), runner));

            let report = tauri::async_runtime::block_on(async {
                dsh_supervisor::run_probe(
                    dsh_supervisor::extract_bin_token(bin_override.as_deref()).as_deref(),
                    path_node.as_deref(),
                    &launch.program,
                )
                .await
            });
            info!(
                version = ?report.version,
                source = ?report.version_source,
                engine_ok = report.engine_ok,
                dsh_reachable = report.dsh_reachable,
                failure = ?report.failure,
                "preflight complete"
            );

            if report.engine_ok {
                let supervisor = tauri::async_runtime::block_on(async {
                    Supervisor::start(launch, RestartPolicy::default())
                });
                app.manage(DaemonState {
                    supervisor: supervisor.clone(),
                });

                let home = app
                    .get_webview_window("main")
                    .and_then(|window| window.url().ok())
                    .unwrap_or_else(|| Url::parse("tauri://localhost").expect("static URL"));

                let handle = app.handle().clone();
                tauri::async_runtime::spawn(forward_task(handle, supervisor, home));
            } else {
                warn!(
                    failure = ?report.failure,
                    "preflight mismatch; supervisor not started"
                );
                // Preflight mismatch: do NOT start the supervisor, do NOT
                // navigate the webview. Push the report to the splash so it
                // can render a guided recovery card.
                let _ = app.emit("preflight://report", &report);
            }

            install_tray(app).map_err(|e| -> Box<dyn std::error::Error> { Box::new(e) })?;

            // 给 splash（首屏）注入一次快捷键监听；守护进程就绪后
            // 主窗口 navigate 到 dsh 时由 Builder.on_page_load 回调重新注入。
            if let Some(main) = app.get_webview_window("main") {
                install_palette_shortcut_on_webview(main.as_ref());
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            daemon_status,
            daemon_log_tail,
            daemon_restart,
            daemon_stop,
            open_plugins_window,
            show_main_window,
            app_quit,
            plugin_install,
            plugin_remove,
            plugin_tasks_list,
            plugin_set_enabled,
            preflight_check,
            dsh_api_call
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app_handle, event| {
        if let tauri::RunEvent::ExitRequested { code, api, .. } = event {
            if code.is_none() {
                // Window-close-triggered exit: keep the process alive so the
                // tray remains usable. Hide the main window to mirror the
                // "minimize to tray" behavior the user asked for.
                api.prevent_exit();
                if let Some(window) = app_handle.get_webview_window("main") {
                    let _ = window.hide();
                }
            } else {
                // Real exit (tray "quit" menu, restart, etc.): stop the
                // supervisor and let the process terminate.
                if let Some(state) = app_handle.try_state::<DaemonState>() {
                    state.supervisor.stop();
                }
            }
        }
    });
}

/// Initialize the `tracing` subscriber.
///
/// Verbosity is driven by the `DSH_CLIENT_LOG` env var (same syntax as
/// `RUST_LOG`: `dsh_client_lib=debug,dsh_supervisor=info`). When unset, the
/// release build stays quiet at `warn` level — diagnostics only surface when
/// the operator opts in.
fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_env("DSH_CLIENT_LOG")
        .unwrap_or_else(|_| EnvFilter::new("warn"));
    let _ = fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
}

/// Install the system tray icon, its right-click menu, and the left-click
/// handler that opens the command palette. Reuses `app.default_window_icon()`
/// so no extra icon asset is required.
fn install_tray(app: &tauri::App) -> tauri::Result<()> {
    let show_palette_item =
        MenuItemBuilder::with_id("show_palette", "打开命令面板").build(app)?;
    let show_plugins_item =
        MenuItemBuilder::with_id("show_plugins", "打开插件管理").build(app)?;
    let show_main_item = MenuItemBuilder::with_id("show_main", "打开主窗口").build(app)?;
    let restart_item = MenuItemBuilder::with_id("restart", "重启 dsh 守护进程").build(app)?;
    let quit_item = MenuItemBuilder::with_id("quit", "退出 DeepSeek Harness").build(app)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;

    let tray_menu = MenuBuilder::new(app)
        .items(&[
            &show_palette_item,
            &show_plugins_item,
            &show_main_item,
            &sep1,
            &restart_item,
            &sep2,
            &quit_item,
        ])
        .build()?;

    TrayIconBuilder::with_id("dsh-tray")
        .icon(
            app.default_window_icon()
                .cloned()
                .ok_or_else(|| tauri::Error::AssetNotFound("default window icon".into()))?,
        )
        .tooltip("DeepSeek Harness")
        .menu(&tray_menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show_palette" => open_palette(app),
            "show_plugins" => {
                let _ = show_window(app, "plugins");
            }
            "show_main" => {
                let _ = show_window(app, "main");
            }
            "restart" => {
                if let Some(state) = app.try_state::<DaemonState>() {
                    state.supervisor.restart();
                }
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                open_palette(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

/// 注入到主窗口 webview 的 keydown 监听器脚本。
/// 命中 `Ctrl+Shift+P` 时阻止默认行为并调用 `open_plugins_window`。
/// 与系统级全局快捷键不同，监听器只在该 webview 拿到焦点时生效，
/// 不会和 VS Code / PowerToys 等进程抢键。
///
/// 注意：必须用 `__TAURI_INTERNALS__.invoke` 而不是 `__TAURI__.core.invoke`。
/// 前者是 Tauri 2 webview 始终注入的宿主内部句柄（不受 `withGlobalTauri`
/// 影响）；后者仅在 `withGlobalTauri: true` 时由 `withGlobalTauri.js` 注入，
/// 而本仓库配置是 `false`，且 dsh 上游页面也不归我们控制——主窗口
/// navigate 到 dsh 之后 `window.__TAURI__` 根本不存在，调它会抛
/// ReferenceError，触发浏览器默认 Ctrl+Shift+P 行为（Chrome 快捷键
/// 面板），用户感受就是"快捷键没反应"。`__TAURI_INTERNALS__` 同样
/// 不受 dsh 自己 JS 影响，且只对宿主可见，安全。
const PALETTE_SHORTCUT_SCRIPT: &str = r#"
(function () {
  if (window.__dsh_palette_listener_installed) return;
  window.__dsh_palette_listener_installed = true;
  window.addEventListener('keydown', function (e) {
    if (e.ctrlKey && e.shiftKey && !e.altKey && !e.metaKey &&
        (e.key === 'P' || e.key === 'p')) {
      e.preventDefault();
      e.stopPropagation();
      // 用宿主内部 IPC 句柄发起命令；命令无参，必须显式传 `{}`，
      // 否则 Tauri 内部把 undefined 当 payload 处理会序列化失败。
      window.__TAURI_INTERNALS__.invoke('open_plugins_window', {})
        .catch(function (err) { console.error('[dsh] open_plugins_window failed', err); });
    }
  }, true);
})();
"#;

/// 把 `PALETTE_SHORTCUT_SCRIPT` 注入到指定 webview 的当前文档。
/// 注入脚本自身用 `__dsh_palette_listener_installed` 做幂等，
/// 所以同一文档里多次调用是安全的。
fn install_palette_shortcut_on_webview(webview: &tauri::Webview) {
    if let Err(err) = webview.eval(PALETTE_SHORTCUT_SCRIPT) {
        warn!(?err, "failed to inject palette shortcut listener");
    }
}
