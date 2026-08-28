//! dsh-client application layer: a thin command surface plus event
//! forwarding. Mechanism only — every policy decision stays in dsh.

use dsh_supervisor::{self, LogStream, RestartPolicy, Supervisor, SupervisorEvent};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Url, WebviewWindow};
use tracing::{debug, error, info, warn};

/// Managed application state: the supervisor handle.
#[derive(Clone)]
struct DaemonState {
    supervisor: Supervisor,
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_tracing();

    let app = tauri::Builder::default()
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
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            daemon_status,
            daemon_log_tail,
            daemon_restart,
            daemon_stop,
            open_plugins_window,
            preflight_check,
            dsh_api_call
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app_handle, event| {
        if let tauri::RunEvent::ExitRequested { .. } = event {
            if let Some(state) = app_handle.try_state::<DaemonState>() {
                state.supervisor.stop();
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
