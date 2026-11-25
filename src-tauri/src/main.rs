use ai_usage_monitor::billing::{BillingStore, ReportingClient};
use ai_usage_monitor::{LocalStatus, Service, Snapshot, SystemDiscovery};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{TrayIcon, TrayIconBuilder},
    Manager, State,
};

type Backend = Service<SystemDiscovery, LocalStatus>;
#[derive(Default)]
struct Authentication {
    manager: ai_usage_monitor::auth::AuthManager,
    gate: tokio::sync::Mutex<()>,
}
fn quit_after_native_auth(app: &tauri::AppHandle) {
    let app = app.clone();
    // The auth gate can be held by a command awaiting the GUI's tray update.
    tauri::async_runtime::spawn(async move {
        if let Some(auth) = app.try_state::<Authentication>() {
            let _guard = auth.gate.lock().await;
            auth.manager.cancel(None).await;
        }
        app.exit(0);
    });
}
struct Panel {
    icon: Option<TrayIcon<tauri::Wry>>,
    menu: Menu<tauri::Wry>,
}

fn show_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

#[tauri::command]
fn hide_to_tray(app: tauri::AppHandle) -> Result<(), String> {
    if !app
        .try_state::<Panel>()
        .is_some_and(|panel| panel.icon.is_some())
    {
        return Err("Desktop panel icon unavailable; keep the application window open.".into());
    }
    app.get_webview_window("main")
        .ok_or("Application window unavailable.")?
        .hide()
        .map_err(|_| "Could not hide the application window.".into())
}

fn publish_panel(app: &tauri::AppHandle) -> Result<(), String> {
    queue_panel_update(app, |_| {})
}

const PANEL_UPDATE_ERROR: &str = "Panel status update unavailable.";

fn queue_panel_update(
    app: &tauri::AppHandle,
    complete: impl FnOnce(Result<(), String>) + Send + 'static,
) -> Result<(), String> {
    let handle = app.clone();
    // Schedule first, then read current data on the GUI thread. Never carry a
    // private snapshot across an await or hold its lock across cross-thread GUI dispatch.
    app.run_on_main_thread(move || {
        let backend = handle.state::<Backend>();
        let update = backend.with_snapshot(|snapshot| -> Result<(), ()> {
            let Some(tray) = handle.tray_by_id("monitor-panel") else {
                return Ok(());
            };
            let tooltip_updated = tray
                .set_tooltip(Some(ai_usage_monitor::panel::tooltip(snapshot)))
                .is_ok();
            // Linux cannot replace an attached tray menu. Keep its three action items.
            let menu = &handle.state::<Panel>().menu;
            let mut removed = true;
            for old in menu.items().map_err(|_| ())?.iter().skip(3) {
                removed &= menu.remove(old).is_ok();
            }
            if !removed {
                return Err(());
            }
            for text in ai_usage_monitor::panel::quota_rows(snapshot) {
                let item = MenuItem::new(&handle, text, false, None::<&str>).map_err(|_| ())?;
                menu.append(&item).map_err(|_| ())?;
            }
            tooltip_updated.then_some(()).ok_or(())
        });
        complete(update.map_err(|_| PANEL_UPDATE_ERROR.into()));
    })
    .map_err(|_| PANEL_UPDATE_ERROR.into())
}

async fn publish_panel_and_wait(app: &tauri::AppHandle) -> Result<(), String> {
    let (send, receive) = tokio::sync::oneshot::channel();
    queue_panel_update(app, move |result| {
        let _ = send.send(result);
    })?;
    receive.await.map_err(|_| PANEL_UPDATE_ERROR.to_string())?
}

fn scan_and_publish(app: &tauri::AppHandle, backend: &Backend) -> Snapshot {
    let snapshot = backend.scan_agents();
    let _ = publish_panel(app);
    snapshot
}

async fn refresh_and_publish(
    app: &tauri::AppHandle,
    backend: &Backend,
    billing: &BillingStore<ReportingClient>,
) {
    tokio::join!(
        backend.refresh_usage_with(|| {
            let _ = publish_panel(app);
        }),
        billing.refresh()
    );
}

async fn refresh_panel(app: tauri::AppHandle) {
    let backend = app.state::<Backend>();
    let billing = app.state::<BillingStore<ReportingClient>>();
    scan_and_publish(&app, &backend);
    refresh_and_publish(&app, &backend, &billing).await;
}

#[tauri::command]
fn scan_agents(
    state: State<'_, Backend>,
    billing: State<'_, BillingStore<ReportingClient>>,
    app: tauri::AppHandle,
) -> Snapshot {
    decorate(scan_and_publish(&app, &state), &billing)
}
#[tauri::command]
async fn refresh_usage(
    state: State<'_, Backend>,
    billing: State<'_, BillingStore<ReportingClient>>,
    app: tauri::AppHandle,
) -> Result<Snapshot, String> {
    refresh_and_publish(&app, &state, &billing).await;
    Ok(decorate(state.with_snapshot(Clone::clone), &billing))
}
#[tauri::command]
async fn set_permission(
    state: State<'_, Backend>,
    agent_id: String,
    allowed: bool,
    billing: State<'_, BillingStore<ReportingClient>>,
    app: tauri::AppHandle,
    auth: State<'_, Authentication>,
) -> Result<Snapshot, String> {
    let (result, publication) = {
        let _guard = auth.gate.lock().await;
        let result = state.set_permission(&agent_id, allowed);
        // A failed durable revoke still clears in-memory permission and data.
        let publication = publish_panel_and_wait(&app).await;
        if !allowed && auth.manager.active_tool().await.as_deref() == Some(agent_id.as_str()) {
            auth.manager.cancel(None).await;
            state.end_auth(&agent_id);
        }
        (result, publication)
    };
    result?;
    if !allowed {
        state.wait_for_status_idle(&agent_id).await;
    }
    let mut snapshot = decorate(state.with_snapshot(Clone::clone), &billing);
    if publication.is_err() {
        snapshot.warnings.push("Permission was updated, but panel status could not refresh. Quit the app to remove its indicator.".into());
    }
    Ok(snapshot)
}

#[tauri::command]
async fn auth_methods(
    agent_id: String,
    state: State<'_, Backend>,
    auth: State<'_, Authentication>,
    app: tauri::AppHandle,
) -> Result<Vec<ai_usage_monitor::auth::AuthChoice>, String> {
    if !scan_and_publish(&app, &state)
        .agents
        .iter()
        .any(|a| a.id == agent_id && a.installed)
    {
        return Err("Installed integration required.".into());
    }
    auth.manager.methods(&agent_id).await
}
#[tauri::command]
async fn auth_start(
    agent_id: String,
    choice_id: String,
    state: State<'_, Backend>,
    auth: State<'_, Authentication>,
    app: tauri::AppHandle,
) -> Result<ai_usage_monitor::auth::AuthView, String> {
    let pause = {
        let _guard = auth.gate.lock().await;
        if let Some(old) = auth.manager.active_tool().await {
            auth.manager.cancel(None).await;
            state.end_auth(&old);
        }
        state.prepare_auth(&agent_id)
    };
    let publication = publish_panel_and_wait(&app).await;
    let pause = pause?;
    publication?;
    state.wait_for_status_idle(&agent_id).await;
    let _guard = auth.gate.lock().await;
    if !pause.claim() {
        return Err("Native sign-in was cancelled before it started.".into());
    }
    match auth.manager.start(&agent_id, &choice_id).await {
        Ok(v) => Ok(v),
        Err(e) => {
            state.end_auth(&agent_id);
            Err(e)
        }
    }
}
#[tauri::command]
async fn auth_poll(
    attempt_id: String,
    state: State<'_, Backend>,
    auth: State<'_, Authentication>,
    app: tauri::AppHandle,
) -> Result<ai_usage_monitor::auth::AuthView, String> {
    let _guard = auth.gate.lock().await;
    if auth.manager.active_id().await.as_deref() == Some(&attempt_id) {
        if let Some(tool) = auth.manager.active_tool().await {
            if !scan_and_publish(&app, &state)
                .agents
                .iter()
                .any(|a| a.id == tool && a.installed)
            {
                auth.manager.cancel(Some(&attempt_id)).await;
                state.end_auth(&tool);
                return Err(
                    "Integration is no longer installed; native sign-in was cancelled.".into(),
                );
            }
        }
    }
    let v = auth.manager.poll(&attempt_id).await?;
    if [
        "authenticated",
        "finished",
        "failed",
        "expired",
        "cancelled",
    ]
    .contains(&v.progress.state.as_str())
    {
        state.end_auth(&v.tool_id);
    }
    Ok(v)
}
#[tauri::command]
async fn auth_submit(
    attempt_id: String,
    code: String,
    auth: State<'_, Authentication>,
) -> Result<(), String> {
    use zeroize::Zeroizing;
    let code = Zeroizing::new(code);
    let _guard = auth.gate.lock().await;
    auth.manager.submit(&attempt_id, &code).await
}
#[tauri::command]
async fn auth_cancel(
    attempt_id: String,
    state: State<'_, Backend>,
    auth: State<'_, Authentication>,
) -> Result<(), String> {
    let _guard = auth.gate.lock().await;
    if auth.manager.active_id().await.as_deref() == Some(&attempt_id) {
        let tool = auth.manager.active_tool().await;
        auth.manager.cancel(Some(&attempt_id)).await;
        if let Some(tool) = tool {
            state.end_auth(&tool);
        }
    }
    Ok(())
}
#[tauri::command]
async fn auth_open_browser(
    attempt_id: String,
    auth: State<'_, Authentication>,
) -> Result<(), String> {
    let url = auth.manager.browser_url(&attempt_id).await?;
    #[cfg(target_os = "linux")]
    let mut command = tokio::process::Command::new("/usr/bin/xdg-open");
    #[cfg(target_os = "macos")]
    let mut command = tokio::process::Command::new("/usr/bin/open");
    #[cfg(target_os = "windows")]
    let mut command = {
        let root = std::env::var_os("SystemRoot")
            .map(std::path::PathBuf::from)
            .filter(|p| p.is_absolute())
            .ok_or("Windows browser launcher unavailable.")?;
        let mut c = tokio::process::Command::new(root.join("System32/rundll32.exe"));
        c.arg("url.dll,FileProtocolHandler");
        c
    };
    command
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), command.status()).await;
    match result {
        Ok(Ok(s)) if s.success() => Ok(()),
        _ => Err("Could not open the sign-in page. Use the displayed provider URL.".into()),
    }
}

fn decorate(mut snapshot: Snapshot, billing: &BillingStore<ReportingClient>) -> Snapshot {
    snapshot.billing = billing.reports();
    snapshot
}

#[tauri::command]
async fn connect_billing(
    state: State<'_, Backend>,
    billing: State<'_, BillingStore<ReportingClient>>,
    provider: String,
    credential: String,
    app: tauri::AppHandle,
) -> Result<Snapshot, String> {
    let snapshot = scan_and_publish(&app, &state);
    let id = match provider.as_str() {
        "openai" => "codex",
        "anthropic" => "claude",
        _ => return Err("Unsupported reporting provider.".into()),
    };
    if !snapshot.agents.iter().any(|a| a.id == id && a.installed) {
        return Err("A matching installed AI app is required.".into());
    }
    billing.connect(&provider, credential)?;
    Ok(decorate(scan_and_publish(&app, &state), &billing))
}

#[tauri::command]
fn disconnect_billing(
    state: State<'_, Backend>,
    billing: State<'_, BillingStore<ReportingClient>>,
    provider: String,
    app: tauri::AppHandle,
) -> Result<Snapshot, String> {
    billing.disconnect(&provider)?;
    Ok(decorate(scan_and_publish(&app, &state), &billing))
}

#[tauri::command]
fn usage_storage_directory(window: tauri::WebviewWindow) -> Result<String, String> {
    let unavailable = || "The local storage directory is unavailable.".to_string();
    if window.label() != "main" {
        return Err(unavailable());
    }
    window
        .app_handle()
        .path()
        .app_local_data_dir()
        .map_err(|_| unavailable())?
        .into_os_string()
        .into_string()
        .map_err(|_| unavailable())
}

#[tauri::command]
async fn read_process_control(
    state: State<'_, Backend>,
    monitor: State<'_, ai_usage_monitor::process_monitor::ProcessMonitor>,
    app: tauri::AppHandle,
) -> Result<ai_usage_monitor::ProcessControlSnapshot, String> {
    if state.with_snapshot(|s| s.agents.is_empty()) {
        scan_and_publish(&app, &state);
    }
    Ok(monitor.sample(state.approved_installed_agents()).await)
}

fn main() {
    // Source: https://v2.tauri.app/learn/system-tray/
    tauri::Builder::default()
        .manage(BillingStore::new(ReportingClient))
        .manage(Authentication::default())
        .manage(ai_usage_monitor::process_monitor::ProcessMonitor::default())
        .invoke_handler(tauri::generate_handler![
            scan_agents,
            refresh_usage,
            set_permission,
            connect_billing,
            disconnect_billing,
            usage_storage_directory,
            read_process_control,
            hide_to_tray,
            auth_methods,
            auth_start,
            auth_poll,
            auth_submit,
            auth_cancel,
            auth_open_browser
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let available = window
                    .app_handle()
                    .try_state::<Panel>()
                    .is_some_and(|panel| panel.icon.is_some());
                if window.label() == "main"
                    && ai_usage_monitor::panel::close_hides_window(available)
                {
                    api.prevent_close();
                    let _ = window.hide();
                } else if window.label() == "main" {
                    api.prevent_close();
                    quit_after_native_auth(window.app_handle());
                }
            }
        })
        .setup(|app| {
            let storage = app.path().app_local_data_dir()?;
            app.manage(Service::with_storage(
                SystemDiscovery,
                LocalStatus,
                &storage,
            ));
            let open = MenuItem::with_id(app, "open", "Open EZTYN AI MONITOR", true, None::<&str>)?;
            let refresh =
                MenuItem::with_id(app, "refresh", "Refresh approved tools", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit EZTYN AI MONITOR", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &refresh, &quit])?;
            let tray = TrayIconBuilder::with_id("monitor-panel")
                .tooltip("EZTYN AI MONITOR")
                .menu(&menu)
                .show_menu_on_left_click(true)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "quit" => quit_after_native_auth(app),
                    "open" => show_window(app),
                    "refresh" => {
                        let app = app.clone();
                        tauri::async_runtime::spawn(async move {
                            refresh_panel(app).await;
                        });
                    }
                    _ => {}
                });
            let tray = app
                .default_window_icon()
                .and_then(|icon| tray.icon(icon.clone()).build(app).ok());
            // The normal window remains usable on desktops without tray support.
            if tray.is_none() {
                eprintln!("System tray unavailable; use the application window.");
            } else if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_skip_taskbar(true);
            }
            app.manage(Panel { icon: tray, menu });
            // The backend, not the hidden webview, owns background monitoring.
            // Service/BillingStore preserve consent, generation guards and per-source caches.
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(45)).await;
                    refresh_panel(handle.clone()).await;
                }
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("Unable to launch desktop application");
}
