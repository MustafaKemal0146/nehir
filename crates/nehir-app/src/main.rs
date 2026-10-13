// Nehir — masaüstü kabuğu (Tauri). Arayüz ui/ klasöründe; indirme mantığı nehir-core'da.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use nehir_core::manager::{Event, Manager as Downloads};
use nehir_core::model::{Kind, NewDownload};
use nehir_core::settings::{Paths, Settings};
use nehir_core::{bridge, native, nettune, video};
use serde_json::json;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_notification::NotificationExt;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

struct AppState {
    downloads: Downloads,
    bridge: Mutex<Option<CancellationToken>>,
    listening: Arc<AtomicBool>,
}

type Res<T> = Result<T, String>;

// ---------- Komutlar ----------

#[tauri::command]
fn list_downloads(state: State<AppState>) -> Vec<nehir_core::model::Download> {
    state.downloads.list()
}

#[tauri::command]
fn add_downloads(state: State<AppState>, urls: Vec<String>, video: bool, quality: Option<String>) -> Res<()> {
    let mut last_err = None;
    for url in urls {
        let req = NewDownload { url, kind: if video { Kind::Video } else { Kind::File }, quality: quality.clone().filter(|_| video), ..Default::default() };
        if let Err(e) = state.downloads.add(req) {
            last_err = Some(e);
        }
    }
    last_err.map_or(Ok(()), Err)
}

fn parse_id(id: &str) -> Res<Uuid> {
    Uuid::parse_str(id).map_err(|e| e.to_string())
}

#[tauri::command]
fn pause(state: State<AppState>, id: String) -> Res<()> {
    state.downloads.pause(parse_id(&id)?);
    Ok(())
}
#[tauri::command]
fn resume(state: State<AppState>, id: String) -> Res<()> {
    state.downloads.resume(parse_id(&id)?);
    Ok(())
}
#[tauri::command]
fn cancel(state: State<AppState>, id: String) -> Res<()> {
    state.downloads.cancel(parse_id(&id)?);
    Ok(())
}
#[tauri::command]
fn remove(state: State<AppState>, id: String, delete_file: bool) -> Res<()> {
    state.downloads.remove(parse_id(&id)?, delete_file);
    Ok(())
}
#[tauri::command]
fn pause_all(state: State<AppState>) {
    state.downloads.pause_all();
}
#[tauri::command]
fn resume_all(state: State<AppState>) {
    state.downloads.resume_all();
}
#[tauri::command]
fn clear_finished(state: State<AppState>) {
    state.downloads.clear_finished();
}

#[tauri::command]
fn get_settings(state: State<AppState>) -> Settings {
    state.downloads.settings()
}

#[tauri::command]
fn set_settings(app: AppHandle, state: State<AppState>, settings: Settings) -> Settings {
    let old = state.downloads.settings();
    let new = state.downloads.update_settings(settings);
    if new.port != old.port {
        start_bridge(&app);
    }
    if new.autostart != old.autostart {
        set_autostart(new.autostart);
    }
    new
}

fn extension_dir() -> Option<PathBuf> {
    let mut c: Vec<PathBuf> = Vec::new();
    if let Some(p) = std::env::var_os("NEHIR_EXTENSION_DIR") {
        c.push(p.into());
    }
    c.push("/usr/share/nehir/extension/chrome".into());
    if let Ok(exe) = std::env::current_exe() {
        // geliştirme: target/{debug,release}/nehir → ../../extension/dist/chrome
        for up in exe.ancestors().skip(1).take(4) {
            c.push(up.join("extension/dist/chrome"));
        }
    }
    c.into_iter().find(|p| p.join("manifest.json").exists())
}

fn native_host_binary() -> Option<PathBuf> {
    let mut c: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            c.push(dir.join("nehir-native-host"));
        }
    }
    c.push("/usr/lib/nehir/nehir-native-host".into());
    c.push("/usr/bin/nehir-native-host".into());
    c.into_iter().find(|p| p.exists())
}

#[tauri::command]
fn get_status(state: State<AppState>) -> serde_json::Value {
    let s = state.downloads.settings();
    json!({
        "version": env!("CARGO_PKG_VERSION"),
        "ytdlp": state.downloads.ytdlp_path().is_some(),
        "port": s.port,
        "language": s.resolved_language(),
        "listening": state.listening.load(Ordering::SeqCst),
        "extension_path": extension_dir().map(|p| p.to_string_lossy().into_owned()),
        "extensions_seen": state.downloads.extensions_seen(),
    })
}

fn xdg_open(path: &std::path::Path) -> Res<()> {
    Command::new("xdg-open")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("xdg-open başlatılamadı: {e}"))
}

#[tauri::command]
fn open_file(state: State<AppState>, id: String) -> Res<()> {
    let d = state.downloads.get(parse_id(&id)?).ok_or("kayıt bulunamadı")?;
    xdg_open(&d.final_path())
}

#[tauri::command]
fn open_folder(state: State<AppState>, id: String) -> Res<()> {
    let d = state.downloads.get(parse_id(&id)?).ok_or("kayıt bulunamadı")?;
    xdg_open(&d.dir)
}

#[tauri::command]
async fn install_ytdlp(state: State<'_, AppState>) -> Res<()> {
    let client = state.downloads.http_client();
    video::install_ytdlp(&client, state.downloads.paths()).await.map(|_| ())
}

#[tauri::command]
async fn pick_folder(app: AppHandle) -> Option<String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().pick_folder(move |p| {
        let _ = tx.send(p.map(|p| p.to_string()));
    });
    rx.await.ok().flatten()
}

#[tauri::command]
fn register_native_host() -> Res<Vec<String>> {
    let host = native_host_binary().ok_or("nehir-native-host bulunamadı")?;
    let home = dirs::home_dir().ok_or("ev dizini bulunamadı")?;
    let done = native::register_for_user(&home, &host, &[native::CHROME_EXTENSION_ID.to_string()]);
    Ok(done.into_iter().map(|r| r.browser.to_string()).collect())
}

#[tauri::command]
fn net_inspect() -> nettune::NetReport {
    nettune::inspect()
}

fn net_parts(tcp: bool, wifi: bool) -> Vec<&'static str> {
    let mut v = Vec::new();
    if tcp {
        v.push("tcp");
    }
    if wifi {
        v.push("wifi");
    }
    v
}

/// Ağ ayarlarını uygular (yönetici parolası penceresi açılır). Yalnızca kullanıcının tıklamasıyla çağrılır.
#[tauri::command]
async fn net_apply(tcp: bool, wifi: bool) -> Res<()> {
    let parts = net_parts(tcp, wifi);
    tauri::async_runtime::spawn_blocking(move || nettune::run_helper("apply", &parts)).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn net_revert(tcp: bool, wifi: bool) -> Res<()> {
    let parts = net_parts(tcp, wifi);
    tauri::async_runtime::spawn_blocking(move || nettune::run_helper("revert", &parts)).await.map_err(|e| e.to_string())?
}

#[tauri::command]
fn copy_text(app: AppHandle, text: String) -> Res<()> {
    app.clipboard().write_text(text).map_err(|e| e.to_string())
}

#[tauri::command]
fn read_clipboard(app: AppHandle) -> String {
    app.clipboard().read_text().unwrap_or_default()
}

// ---------- Köprü ----------

fn start_bridge(app: &AppHandle) {
    let state = app.state::<AppState>();
    if let Some(old) = state.bridge.lock().unwrap().take() {
        old.cancel();
    }
    let token = CancellationToken::new();
    *state.bridge.lock().unwrap() = Some(token.clone());
    let manager = state.downloads.clone();
    let listening = state.listening.clone();
    let port = manager.settings().port;
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // Port değişiminde eski dinleyicinin kapanması için kısa bekleme ve birkaç deneme.
        let mut listener = None;
        for _ in 0..10 {
            match bridge::bind(port).await {
                Ok(l) => {
                    listener = Some(l);
                    break;
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(300)).await,
            }
        }
        let Some(listener) = listener else {
            tracing::warn!("127.0.0.1:{port} dinlenemiyor");
            listening.store(false, Ordering::SeqCst);
            let _ = app.emit("bridge-error", port);
            return;
        };
        listening.store(true, Ordering::SeqCst);
        tracing::info!("köprü 127.0.0.1:{port} üzerinde dinliyor");
        let _ = bridge::serve(listener, manager, token).await;
        listening.store(false, Ordering::SeqCst);
    });
}

// ---------- Yardımcılar ----------

/// Arayüz dili bilgisayarın diline göre (Türkçe ise Türkçe, değilse İngilizce); ayardan değiştirilebilir.
fn is_turkish(settings: &Settings) -> bool {
    settings.resolved_language() == "tr"
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn autostart_file() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("autostart/nehir.desktop"))
}

fn set_autostart(on: bool) {
    let Some(file) = autostart_file() else { return };
    if !on {
        let _ = std::fs::remove_file(file);
        return;
    }
    let exe = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| "nehir".into());
    let entry = format!(
        "[Desktop Entry]\nType=Application\nName=Nehir\nExec=\"{exe}\" --background\nIcon=nehir\nX-GNOME-Autostart-enabled=true\nTerminal=false\n"
    );
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(file, entry);
}

fn looks_capturable(url: &str, settings: &Settings) -> bool {
    let Ok(u) = url::Url::parse(url) else { return false };
    if !matches!(u.scheme(), "http" | "https") {
        return false;
    }
    let host = u.host_str().unwrap_or("").to_lowercase();
    const VIDEO: &[&str] = &["youtube.com", "youtu.be", "vimeo.com", "dailymotion.com", "twitch.tv", "tiktok.com", "bilibili.com"];
    if VIDEO.iter().any(|v| host == *v || host.ends_with(&format!(".{v}"))) {
        return true;
    }
    let ext = u.path().rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    !ext.is_empty() && settings.capture_extensions.iter().any(|e| *e == ext)
}

fn spawn_clipboard_watcher(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut last = String::new();
        loop {
            tokio::time::sleep(Duration::from_millis(1500)).await;
            let state = app.state::<AppState>();
            let settings = state.downloads.settings();
            if !settings.monitor_clipboard {
                continue;
            }
            let Ok(text) = app.clipboard().read_text() else { continue };
            let text = text.trim().to_string();
            if text.is_empty() || text == last || text.contains('\n') || text.len() > 2048 {
                continue;
            }
            last = text.clone();
            if looks_capturable(&text, &settings) {
                let _ = app.emit("clipboard-url", text);
            }
        }
    });
}

fn spawn_event_forwarder(app: AppHandle) {
    let mut rx = app.state::<AppState>().downloads.subscribe();
    tauri::async_runtime::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(Event::Updated(d)) => {
                    let _ = app.emit("download-updated", d);
                }
                Ok(Event::Removed(id)) => {
                    let _ = app.emit("download-removed", id);
                }
                Ok(Event::Settings(s)) => {
                    let _ = app.emit("settings-changed", s);
                }
                Ok(Event::Completed(d)) => {
                    let settings = app.state::<AppState>().downloads.settings();
                    if settings.notifications {
                        let tr = is_turkish(&settings);
                        let _ = app
                            .notification()
                            .builder()
                            .title(if tr { "İndirme tamamlandı" } else { "Download completed" })
                            .body(&d.file_name)
                            .show();
                    }
                    let _ = app.emit("download-completed", d);
                }
                Ok(Event::Failed(d)) => {
                    let settings = app.state::<AppState>().downloads.settings();
                    if settings.notifications {
                        let tr = is_turkish(&settings);
                        let _ = app
                            .notification()
                            .builder()
                            .title(if tr { "İndirme başarısız" } else { "Download failed" })
                            .body(&d.file_name)
                            .show();
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    });
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let tr = is_turkish(&app.state::<AppState>().downloads.settings());
    let (t_show, t_add, t_pause, t_resume, t_quit) = if tr {
        ("Nehir'i göster", "İndirme ekle…", "Tümünü duraklat", "Tümünü sürdür", "Çık")
    } else {
        ("Show Nehir", "Add download…", "Pause all", "Resume all", "Quit")
    };
    let show = MenuItem::with_id(app, "show", t_show, true, None::<&str>)?;
    let add = MenuItem::with_id(app, "add", t_add, true, None::<&str>)?;
    let pause = MenuItem::with_id(app, "pause", t_pause, true, None::<&str>)?;
    let resume = MenuItem::with_id(app, "resume", t_resume, true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", t_quit, true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&show, &add, &sep, &pause, &resume, &sep, &quit])?;

    let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/128x128.png"))?;
    TrayIconBuilder::with_id("main")
        .icon(icon)
        .tooltip("Nehir")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => show_main(app),
            "add" => {
                show_main(app);
                let _ = app.emit("open-add-dialog", ());
            }
            "pause" => app.state::<AppState>().downloads.pause_all(),
            "resume" => app.state::<AppState>().downloads.resume_all(),
            "quit" => {
                app.state::<AppState>().downloads.pause_all();
                let handle = app.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(500));
                    handle.exit(0);
                });
            }
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

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let background = std::env::args().any(|a| a == "--background");

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_main(app)))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .setup(move |app| {
            let downloads = tauri::async_runtime::block_on(async { Downloads::new(Paths::default_dirs()) });
            app.manage(AppState { downloads, bridge: Mutex::new(None), listening: Arc::new(AtomicBool::new(false)) });
            let handle = app.handle().clone();

            // Kurulu tarayıcılara native messaging kaydı (uygulama kapalıyken eklentinin onu başlatabilmesi için).
            if let (Some(host), Some(home)) = (native_host_binary(), dirs::home_dir()) {
                let done = native::register_for_user(&home, &host, &[native::CHROME_EXTENSION_ID.to_string()]);
                tracing::info!("native host kaydedildi: {}", done.iter().map(|r| r.browser).collect::<Vec<_>>().join(", "));
            }

            start_bridge(&handle);
            spawn_event_forwarder(handle.clone());
            spawn_clipboard_watcher(handle.clone());
            if let Err(e) = build_tray(&handle) {
                tracing::warn!("sistem tepsisi kurulamadı: {e}");
            }
            if let Some(w) = app.get_webview_window("main") {
                if let Ok(icon) = tauri::image::Image::from_bytes(include_bytes!("../icons/icon.png")) {
                    let _ = w.set_icon(icon);
                }
                if !background {
                    let _ = w.show();
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let state = window.app_handle().state::<AppState>();
                if state.downloads.settings().close_to_tray {
                    api.prevent_close();
                    let _ = window.hide();
                } else {
                    state.downloads.pause_all();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            list_downloads, add_downloads, pause, resume, cancel, remove, pause_all, resume_all, clear_finished,
            get_settings, set_settings, get_status, open_file, open_folder, install_ytdlp, pick_folder,
            register_native_host, copy_text, read_clipboard, net_inspect, net_apply, net_revert,
        ])
        .run(tauri::generate_context!())
        .expect("Nehir başlatılamadı");
}
