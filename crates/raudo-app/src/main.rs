//! raudo desktop app: the download service plus its window and tray icon.
//!
//! The app runs the service itself unless another raudo (e.g. `raudo-daemon`)
//! already does; either way the window talks to it over the same local IPC as
//! the CLI and the browser host, so all clients see the same state.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use raudo_ipc::{Client, Event};
use raudo_proto::{AddDownload, DownloadInfo, MediaInfo, Request, Status};
use serde_json::Value;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, WindowEvent};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_opener::OpenerExt;
use tokio::sync::Mutex;

/// The service when this process runs it (None: another raudo does).
struct AppState {
    service: Mutex<Option<raudo_service::Service>>,
}

type CmdResult<T> = Result<T, String>;

async fn client() -> CmdResult<Client> {
    Client::connect("app").await.map_err(|e| e.to_string())
}

fn err(e: raudo_ipc::ClientError) -> String {
    e.to_string()
}

// ---------------------------------------------------------------- commands

#[tauri::command]
async fn list() -> CmdResult<Vec<DownloadInfo>> {
    client().await?.list().await.map_err(err)
}

#[tauri::command]
async fn add(download: AddDownload) -> CmdResult<i64> {
    client().await?.add(download).await.map_err(err)
}

#[tauri::command]
async fn pause(id: i64) -> CmdResult<()> {
    client().await?.pause(id).await.map_err(err)
}

#[tauri::command]
async fn resume(id: i64) -> CmdResult<()> {
    client().await?.resume(id).await.map_err(err)
}

#[tauri::command]
async fn remove(id: i64, delete_file: bool) -> CmdResult<()> {
    client().await?.remove(id, delete_file).await.map_err(err)
}

#[tauri::command]
async fn details(id: i64) -> CmdResult<Value> {
    client().await?.details(id).await.map_err(err)
}

#[tauri::command]
async fn formats(url: String) -> CmdResult<MediaInfo> {
    client().await?.formats(Request { url, ..Default::default() }).await.map_err(err)
}

#[tauri::command]
async fn settings() -> CmdResult<Value> {
    client().await?.settings().await.map_err(err)
}

#[tauri::command]
async fn set_speed_limit(kib: u64) -> CmdResult<()> {
    client().await?.set_speed_limit(kib).await.map_err(err)
}

#[tauri::command]
async fn set_max_concurrent(n: u32) -> CmdResult<()> {
    client().await?.set_max_concurrent(n).await.map_err(err)
}

async fn pause_or_resume_all(pause: bool) -> CmdResult<()> {
    let mut c = client().await?;
    for d in c.list().await.map_err(err)? {
        match (pause, d.status) {
            (true, Status::Downloading | Status::Queued) => c.pause(d.id).await.map_err(err)?,
            (false, Status::Paused) => c.resume(d.id).await.map_err(err)?,
            _ => {}
        }
    }
    Ok(())
}

#[tauri::command]
async fn pause_all() -> CmdResult<()> {
    pause_or_resume_all(true).await
}

#[tauri::command]
async fn resume_all() -> CmdResult<()> {
    pause_or_resume_all(false).await
}

#[tauri::command]
fn default_dir() -> String {
    dirs::download_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join("Downloads")))
        .map(|p| p.display().to_string())
        .unwrap_or_default()
}

#[tauri::command]
async fn pick_folder(app: AppHandle) -> CmdResult<Option<String>> {
    // The dialog blocks until closed: keep it off the async workers.
    tauri::async_runtime::spawn_blocking(move || {
        app.dialog().file().blocking_pick_folder().map(|p| p.to_string())
    })
    .await
    .map_err(|e| e.to_string())
}

/// Opens the file manager on the downloaded file (or its folder).
#[tauri::command]
async fn reveal(app: AppHandle, id: i64) -> CmdResult<()> {
    let d = client().await?.details(id).await.map_err(err)?;
    let path = d.get("path").and_then(Value::as_str).map(PathBuf::from);
    let folder = d.pointer("/info/dir").and_then(Value::as_str).map(PathBuf::from);
    match path.filter(|p| p.exists()) {
        Some(p) => app.opener().reveal_item_in_dir(p).map_err(|e| e.to_string()),
        None => match folder {
            Some(f) => app.opener().open_path(f.display().to_string(), None::<&str>).map_err(|e| e.to_string()),
            None => Err("no folder for this download".into()),
        },
    }
}

#[tauri::command]
fn autostart_enabled(app: AppHandle) -> bool {
    app.autolaunch().is_enabled().unwrap_or(false)
}

#[tauri::command]
fn set_autostart(app: AppHandle, enabled: bool) -> CmdResult<()> {
    let launcher = app.autolaunch();
    if enabled { launcher.enable() } else { launcher.disable() }.map_err(|e| e.to_string())
}

/// Whether this window's process runs the downloads (false: raudo-daemon does).
#[tauri::command]
async fn owns_service(state: tauri::State<'_, AppState>) -> CmdResult<bool> {
    Ok(state.service.lock().await.is_some())
}

// ------------------------------------------------------------ window & tray

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// Pauses and saves the downloads this process runs, then exits.
fn quit(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Some(service) = app.state::<AppState>().service.lock().await.take() {
            service.shutdown().await;
        }
        app.exit(0);
    });
}

fn build_tray(app: &tauri::App) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Abrir raudo", true, None::<&str>)?;
    let pause = MenuItem::with_id(app, "pause_all", "Pausar todas", true, None::<&str>)?;
    let resume = MenuItem::with_id(app, "resume_all", "Reanudar todas", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let quit_item = MenuItem::with_id(app, "quit", "Salir (pausa y guarda las descargas)", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &pause, &resume, &sep, &quit_item])?;
    TrayIconBuilder::with_id("raudo")
        .icon(app.default_window_icon().cloned().expect("bundle icon"))
        .tooltip("raudo")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show_main(app),
            "pause_all" => {
                tauri::async_runtime::spawn(pause_or_resume_all(true));
            }
            "resume_all" => {
                tauri::async_runtime::spawn(pause_or_resume_all(false));
            }
            "quit" => quit(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                show_main(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

/// Forwards service events to the window and notifies finished or failed downloads.
async fn forward_events(app: AppHandle) {
    let mut last: HashMap<i64, Status> = HashMap::new();
    loop {
        if let Ok(mut c) = Client::connect("app-events").await {
            if c.subscribe().await.is_ok() {
                while let Ok(Some(ev)) = c.next_event().await {
                    if let Event::Changed { info } = &ev {
                        let before = last.insert(info.id, info.status);
                        if before.is_some_and(|b| b != info.status) {
                            notify_status(&app, info);
                        }
                    }
                    let _ = app.emit("raudo-event", &ev);
                }
            }
        }
        // Service restarting (or not up yet): try again shortly.
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

fn notify_status(app: &AppHandle, d: &DownloadInfo) {
    let name = d.filename.clone().unwrap_or_else(|| d.url.clone());
    let (title, body) = match d.status {
        Status::Completed => ("Descarga completada", name),
        Status::Error => ("La descarga ha fallado", format!("{name}\n{}", d.error.clone().unwrap_or_default())),
        _ => return,
    };
    let _ = app.notification().builder().title(title).body(body).show();
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    // Started at login, or by the browser host: stay in the tray.
    let background = std::env::args().any(|a| a == "--background");

    tauri::Builder::default()
        // Must be first: a second launch only brings this window forward.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_main(app)))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--background"]),
        ))
        .setup(move |app| {
            let service = match tauri::async_runtime::block_on(raudo_service::Service::start()) {
                Ok(s) => Some(s),
                Err(raudo_service::StartError::AlreadyRunning) => {
                    tracing::info!("another raudo runs the downloads; this window is its client");
                    None
                }
                Err(e) => {
                    tracing::error!("cannot start the download service: {e}");
                    None
                }
            };
            app.manage(AppState { service: Mutex::new(service) });
            build_tray(app)?;
            tauri::async_runtime::spawn(forward_events(app.handle().clone()));
            if !background {
                show_main(app.handle());
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing hides to the tray: downloads keep going.
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            list,
            add,
            pause,
            resume,
            remove,
            details,
            formats,
            settings,
            set_speed_limit,
            set_max_concurrent,
            pause_all,
            resume_all,
            default_dir,
            pick_folder,
            reveal,
            autostart_enabled,
            set_autostart,
            owns_service
        ])
        .run(tauri::generate_context!())
        .expect("raudo failed to start");
}
