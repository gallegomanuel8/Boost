//! Boost desktop app (raudo engine): the download service plus its window and tray icon.
//!
//! The app runs the service itself unless another raudo (e.g. `raudo-daemon`)
//! already does; either way the window talks to it over the same local IPC as
//! the CLI and the browser host, so all clients see the same state.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod browsers;

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
async fn restart(id: i64) -> CmdResult<()> {
    client().await?.restart(id).await.map_err(err)
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

/// Opens the downloaded file with the application the system has for it.
#[tauri::command]
async fn open_file(app: AppHandle, id: i64) -> CmdResult<()> {
    let d = client().await?.details(id).await.map_err(err)?;
    if d.pointer("/info/status").and_then(Value::as_str) != Some("completed") {
        return Err("la descarga no ha terminado".into());
    }
    let path = d.get("path").and_then(Value::as_str).map(PathBuf::from).filter(|p| p.is_file());
    let path = path.ok_or("el fichero ya no está en la carpeta")?;
    app.opener().open_path(path.display().to_string(), None::<&str>).map_err(|e| e.to_string())
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

fn xpi(app: &AppHandle) -> Option<PathBuf> {
    browsers::find_xpi(app.path().resource_dir().ok())
}

/// The raudo extension in each browser: installed or not, and installable.
#[tauri::command]
async fn web_extensions(app: AppHandle) -> Vec<browsers::WebExtension> {
    // Reads several profile folders: off the main thread.
    tauri::async_runtime::spawn_blocking(move || browsers::list(xpi(&app).as_deref()))
        .await
        .unwrap_or_default()
}

#[tauri::command]
fn install_extension(app: AppHandle, browser: String) -> CmdResult<()> {
    if browser == "firefox" {
        return browsers::install_firefox(&xpi(&app).ok_or("No se encuentra el fichero de la extensión")?);
    }
    // Chromium browsers only load it by hand (developer mode → Load unpacked):
    // open its folder so the user can pick it.
    let folder = browsers::list(None)
        .into_iter()
        .find(|x| x.browser == browser)
        .and_then(|x| x.folder)
        .ok_or("No se encuentra la carpeta de la extensión; reinstala Boost")?;
    app.opener().open_path(folder.display().to_string(), None::<&str>).map_err(|e| e.to_string())
}

/// Marks that the welcome guide was seen (per user, not per window).
fn welcome_marker() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("boost").join("welcome-done"))
}

/// Only macOS gets the welcome guide: elsewhere the installer registers Boost
/// with the browsers and offers the extension.
fn first_run() -> bool {
    cfg!(target_os = "macos") && welcome_marker().is_some_and(|m| !m.exists())
}

/// The .app this process runs from (macOS), and whether it sits in an Applications folder.
#[cfg(target_os = "macos")]
fn app_bundle() -> Option<(PathBuf, bool)> {
    let exe = std::env::current_exe().ok()?;
    let bundle = exe.ancestors().nth(3)?.to_path_buf();
    if bundle.extension().is_none_or(|e| e != "app") {
        return None;
    }
    let parent = bundle.parent()?;
    let home_apps = dirs::home_dir().map(|h| h.join("Applications"));
    let installed = parent == std::path::Path::new("/Applications") || Some(parent.to_path_buf()) == home_apps;
    Some((bundle, installed))
}

/// What the welcome guide shows.
#[tauri::command]
fn welcome_state() -> Value {
    #[cfg(target_os = "macos")]
    // Run from the disk image or Downloads (not from a cargo build): offer to move it.
    let needs_move = app_bundle().is_some_and(|(_, installed)| !installed);
    #[cfg(not(target_os = "macos"))]
    let needs_move = false;
    serde_json::json!({
        "first_run": first_run(),
        "platform": std::env::consts::OS,
        "needs_move": needs_move,
        "ytdlp": raudo_service::find_tool("yt-dlp").is_some(),
        "ffmpeg": raudo_service::find_tool("ffmpeg").is_some(),
    })
}

#[tauri::command]
fn finish_welcome() -> CmdResult<()> {
    let marker = welcome_marker().ok_or("no data folder")?;
    if let Some(dir) = marker.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(marker, b"").map_err(|e| e.to_string())
}

/// Copies Boost into /Applications (or ~/Applications when that is not
/// writable) and restarts it from there, so the browsers keep finding it after
/// the disk image is ejected. The original copy is left where it was.
#[tauri::command]
async fn move_to_applications(app: AppHandle) -> CmdResult<()> {
    #[cfg(target_os = "macos")]
    {
        let (bundle, _) = app_bundle().ok_or("Boost no se está ejecutando desde una aplicación")?;
        let system = PathBuf::from("/Applications");
        let parent = if writable(&system) {
            system
        } else {
            let home = dirs::home_dir().ok_or("no home folder")?.join("Applications");
            std::fs::create_dir_all(&home).map_err(|e| e.to_string())?;
            home
        };
        let dest = parent.join("Boost.app");
        if dest.exists() {
            std::fs::remove_dir_all(&dest).map_err(|e| format!("No se pudo reemplazar {}: {e}", dest.display()))?;
        }
        let ok = std::process::Command::new("ditto").arg(&bundle).arg(&dest).status().map_err(|e| e.to_string())?;
        if !ok.success() {
            return Err(format!("No se pudo copiar Boost a {}", parent.display()));
        }
        // The new copy can only start once this one has gone (single instance).
        let relaunch = format!("while kill -0 {} 2>/dev/null; do sleep 0.2; done; open \"$0\"", std::process::id());
        std::process::Command::new("/bin/sh").arg("-c").arg(relaunch).arg(&dest).spawn().map_err(|e| e.to_string())?;
        quit(&app);
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Err("solo en macOS".into())
    }
}

#[cfg(target_os = "macos")]
fn writable(dir: &std::path::Path) -> bool {
    let probe = dir.join(format!(".boost-write-test-{}", std::process::id()));
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(probe);
    ok
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
    let open = MenuItem::with_id(app, "open", "Abrir Boost", true, None::<&str>)?;
    let pause = MenuItem::with_id(app, "pause_all", "Pausar todas", true, None::<&str>)?;
    let resume = MenuItem::with_id(app, "resume_all", "Reanudar todas", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let quit_item = MenuItem::with_id(app, "quit", "Salir (pausa y guarda las descargas)", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &pause, &resume, &sep, &quit_item])?;
    // Bolt only: the full icon's "Boost" lettering is unreadable at tray size.
    let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))?;
    #[cfg(target_os = "macos")]
    let icon = template_icon(&icon);
    let tray = TrayIconBuilder::with_id("raudo").icon(icon);
    // The menu bar tints template images to match its light or dark look.
    #[cfg(target_os = "macos")]
    let tray = tray.icon_as_template(true);
    tray.tooltip("Boost")
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

/// The tray icon's white bolt alone, black on transparent and cropped to it:
/// the menu bar's monochrome style. The yellow tile around it has little blue,
/// the bolt is white, so blue tells them apart (anti-aliased edges included).
#[cfg(target_os = "macos")]
fn template_icon(icon: &tauri::image::Image<'_>) -> tauri::image::Image<'static> {
    let (w, h) = (icon.width() as usize, icon.height() as usize);
    let alpha: Vec<u8> = icon
        .rgba()
        .chunks_exact(4)
        .map(|p| {
            let bolt = (p[2].saturating_sub(120) as u32 * 255 / 135).min(255);
            (bolt * p[3] as u32 / 255) as u8
        })
        .collect();
    // Square around the bolt, with room around it: the menu bar scales the image
    // to its height, and other apps' glyphs fill about 70% of it.
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for (i, _) in alpha.iter().enumerate().filter(|(_, a)| **a > 32) {
        let (x, y) = (i % w, i / w);
        (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
    }
    if x0 > x1 {
        return icon.clone().to_owned();
    }
    let side = ((x1 - x0).max(y1 - y0) + 1) * 10 / 7;
    let (cx, cy) = ((x0 + x1) / 2, (y0 + y1) / 2);
    let mut out = vec![0u8; side * side * 4];
    for ty in 0..side {
        for tx in 0..side {
            let (sx, sy) = ((cx + tx).checked_sub(side / 2), (cy + ty).checked_sub(side / 2));
            if let (Some(sx), Some(sy)) = (sx, sy) {
                if sx < w && sy < h {
                    out[(ty * side + tx) * 4 + 3] = alpha[sy * w + sx];
                }
            }
        }
    }
    tauri::image::Image::new_owned(out, side as u32, side as u32)
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

/// A macOS app is dragged into Applications, with no installer to run: each
/// start tells the browsers where the host inside this bundle is, and leaves
/// the extension where Chromium browsers can load it.
#[cfg(target_os = "macos")]
fn register_with_browsers(app: &AppHandle) {
    let resources = app.path().resource_dir().ok();
    std::thread::spawn(move || {
        if let Err(e) = browsers::sync_extension_folder(resources) {
            tracing::warn!("cannot copy the browser extension: {e}");
        }
        let Some(host) = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("raudo-nmhost"))) else {
            return;
        };
        if !host.is_file() {
            return;
        }
        match std::process::Command::new(&host).arg("register").stdout(std::process::Stdio::null()).status() {
            Ok(s) if s.success() => {}
            other => tracing::warn!("cannot register the browser host: {other:?}"),
        }
    });
}

fn is_background_arg(arg: &str) -> bool {
    arg == "--background" || arg.starts_with("boost:")
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    // Started at login, by the browser host, or by the extension's "launch" link
    // (boost://launch): stay in the tray.
    let background = std::env::args().skip(1).any(|a| is_background_arg(&a));

    tauri::Builder::default()
        // Must be first: a second launch only brings this window forward.
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            if !args.iter().skip(1).any(|a| is_background_arg(a)) {
                show_main(app);
            }
        }))
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
            #[cfg(target_os = "macos")]
            register_with_browsers(app.handle());
            tauri::async_runtime::spawn(forward_events(app.handle().clone()));
            // The welcome guide needs the window, however Boost was started.
            if !background || first_run() {
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
            restart,
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
            open_file,
            reveal,
            autostart_enabled,
            set_autostart,
            owns_service,
            web_extensions,
            install_extension,
            welcome_state,
            finish_welcome,
            move_to_applications
        ])
        .build(tauri::generate_context!())
        .expect("Boost failed to start")
        .run(|app, event| match event {
            // Cmd+Q and the app menu's Quit: pause and save first, as the tray's Quit does
            // (which exits with a code once done).
            tauri::RunEvent::ExitRequested { code: None, api, .. } => {
                api.prevent_exit();
                quit(app);
            }
            // Clicking the Dock icon with the window hidden.
            #[cfg(target_os = "macos")]
            tauri::RunEvent::Reopen { has_visible_windows: false, .. } => show_main(app),
            _ => {}
        });
}
