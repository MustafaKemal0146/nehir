// Nehir — masaüstü kabuğu (Tauri). Arayüz ui/ klasöründe; indirme mantığı nehir-core'da.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use nehir_core::manager::{Event, Manager as Downloads};
use nehir_core::model::{Kind, NewDownload};
use nehir_core::settings::{Paths, Settings};
use nehir_core::{bridge, native, nettune, video};
use serde_json::json;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_dialog::DialogExt;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

struct AppState {
    downloads: Downloads,
    bridge: Mutex<Option<CancellationToken>>,
    listening: Arc<AtomicBool>,
    after: Mutex<AfterAll>,
}

/// "İndirmeler bitince": uyku, kapatma ya da çıkış. Kalıcı değildir (uygulama yeniden açılınca kapalı başlar),
/// böylece unutulmuş bir "kapat" ayarı bilgisayarı beklenmedik anda kapatmaz.
#[derive(Default)]
struct AfterAll {
    action: String,
    /// Kurulduktan sonra en az bir indirme tamamlandı mı? (Boş listeyle kurulunca hemen tetiklenmesin.)
    completed_since: bool,
    /// Geri sayım sürüyorsa: `true` gönderilirse hemen yap, `false` ise vazgeç.
    countdown: Option<tokio::sync::oneshot::Sender<bool>>,
}

const AFTER_ACTIONS: &[&str] = &["none", "sleep", "shutdown", "quit"];
const AFTER_COUNTDOWN_SECS: u64 = 60;

type Res<T> = Result<T, String>;

// ---------- Komutlar ----------

#[tauri::command]
fn list_downloads(state: State<AppState>) -> Vec<nehir_core::model::Download> {
    state.downloads.list()
}

#[tauri::command]
fn add_downloads(state: State<AppState>, urls: Vec<String>, video: bool, quality: Option<String>, checksum: Option<String>) -> Res<()> {
    let checksum = match checksum.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(c) => Some(nehir_core::checksum::normalize(c).ok_or("invalid_checksum")?),
        None => None,
    };
    let single = urls.len() == 1;
    let mut last_err = None;
    for url in urls {
        let req = NewDownload {
            url,
            kind: if video { Kind::Video } else { Kind::File },
            quality: quality.clone().filter(|_| video),
            // Özet tek bir dosyaya aittir; birden çok adreste kullanılmaz.
            checksum: checksum.clone().filter(|_| single),
            ..Default::default()
        };
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
fn verify_checksum(state: State<AppState>, id: String, expected: Option<String>) -> Res<()> {
    state.downloads.verify(parse_id(&id)?, expected)
}

#[tauri::command]
fn get_after_all(state: State<AppState>) -> String {
    let a = state.after.lock().unwrap();
    if a.action.is_empty() { "none".into() } else { a.action.clone() }
}

#[tauri::command]
fn set_after_all(app: AppHandle, state: State<AppState>, action: String) -> Res<String> {
    if !AFTER_ACTIONS.contains(&action.as_str()) {
        return Err("geçersiz işlem".into());
    }
    {
        let mut a = state.after.lock().unwrap();
        if let Some(tx) = a.countdown.take() {
            let _ = tx.send(false);
        }
        a.action = action.clone();
        a.completed_since = false;
    }
    let _ = app.emit("after-all-changed", &action);
    Ok(action)
}

/// Geri sayımı yanıtlar: `now` ise hemen uygular, değilse vazgeçer (işlem "hiçbir şey"e döner).
#[tauri::command]
fn answer_after_all(app: AppHandle, state: State<AppState>, now: bool) {
    let tx = state.after.lock().unwrap().countdown.take();
    if let Some(tx) = tx {
        let _ = tx.send(now);
    }
    if !now {
        state.after.lock().unwrap().action = "none".into();
        let _ = app.emit("after-all-changed", "none");
    }
}

/// Chromium tabanlı tarayıcıda eklentiler sayfasını açar (ilk kurulum sihirbazı için).
#[cfg(unix)]
#[tauri::command]
fn open_extensions_page() -> Res<String> {
    const BROWSERS: &[(&str, &str)] = &[
        ("google-chrome", "Google Chrome"),
        ("google-chrome-stable", "Google Chrome"),
        ("brave-browser", "Brave"),
        ("microsoft-edge", "Microsoft Edge"),
        ("chromium", "Chromium"),
        ("chromium-browser", "Chromium"),
        ("vivaldi", "Vivaldi"),
    ];
    for (bin, name) in BROWSERS {
        let ok = Command::new(bin)
            .arg("chrome://extensions/")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .is_ok();
        if ok {
            return Ok(name.to_string());
        }
    }
    Err("no_browser".into())
}

/// Windows'ta tarayıcılar "App Paths" kaydıyla bulunur; `cmd /C start` bunu çözer ve tarayıcı
/// kurulu değilse başarısız olur (çıkış kodu ≠ 0), böylece sıradakine geçilir.
#[cfg(windows)]
#[tauri::command]
fn open_extensions_page() -> Res<String> {
    use std::os::windows::process::CommandExt;
    const BROWSERS: &[(&str, &str, &str)] = &[
        ("chrome", "Google Chrome", "chrome://extensions/"),
        ("msedge", "Microsoft Edge", "edge://extensions/"),
        ("brave", "Brave", "brave://extensions/"),
        ("vivaldi", "Vivaldi", "vivaldi://extensions/"),
    ];
    for (exe, name, url) in BROWSERS {
        let ok = Command::new("cmd")
            .args(["/C", "start", "", exe, url])
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            return Ok(name.to_string());
        }
    }
    Err("no_browser".into())
}

#[tauri::command]
fn open_extension_folder() -> Res<()> {
    open_path(&extension_dir().ok_or("eklenti klasörü bulunamadı")?)
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
    #[cfg(unix)]
    c.push("/usr/share/nehir/extension/chrome".into());
    if let Ok(exe) = std::env::current_exe() {
        // Windows kurulumu: nehir.exe'nin yanında extension\chrome.
        if let Some(dir) = exe.parent() {
            c.push(dir.join("extension").join("chrome"));
        }
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
            c.push(dir.join(nehir_core::platform::exe_name("nehir-native-host")));
        }
    }
    #[cfg(unix)]
    {
        c.push("/usr/lib/nehir/nehir-native-host".into());
        c.push("/usr/bin/nehir-native-host".into());
    }
    c.into_iter().find(|p| p.exists())
}

/// Native messaging kaydını yapar ve kaydedilen tarayıcıların adlarını döndürür.
/// Linux: tarayıcıların manifest klasörlerine yazar. Windows: manifestleri veri klasörüne yazar,
/// tarayıcıların kayıt defteri anahtarlarını bunlara yöneltir.
fn register_native_hosts() -> Res<Vec<String>> {
    let host = native_host_binary().ok_or("nehir-native-host bulunamadı")?;
    let ids = [native::CHROME_EXTENSION_ID.to_string()];
    #[cfg(unix)]
    let done = {
        let home = dirs::home_dir().ok_or("ev dizini bulunamadı")?;
        native::register_for_user(&home, &host, &ids)
    };
    #[cfg(windows)]
    let done = native::register_for_user(&Paths::default_dirs().data_dir.join("native-messaging"), &host, &ids);
    Ok(done.into_iter().map(|r| r.browser.to_string()).collect())
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
        "net_supported": cfg!(unix),
        "extension_path": extension_dir().map(|p| p.to_string_lossy().into_owned()),
        "extensions_seen": state.downloads.extensions_seen(),
    })
}

fn open_path(path: &std::path::Path) -> Res<()> {
    nehir_core::platform::open_path(path)
}

#[tauri::command]
fn open_file(state: State<AppState>, id: String) -> Res<()> {
    let d = state.downloads.get(parse_id(&id)?).ok_or("kayıt bulunamadı")?;
    open_path(&d.final_path())
}

/// Klasörü açar; dosya varsa Windows'ta Gezgin onu seçili gösterir.
#[tauri::command]
fn open_folder(state: State<AppState>, id: String) -> Res<()> {
    let d = state.downloads.get(parse_id(&id)?).ok_or("kayıt bulunamadı")?;
    let file = d.final_path();
    if file.exists() { nehir_core::platform::reveal_path(&file) } else { open_path(&d.dir) }
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
    register_native_hosts()
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

/// Windows: oturum açılınca başlatma, `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` girdisiyle.
#[cfg(windows)]
fn set_autostart(on: bool) {
    let Ok(exe) = std::env::current_exe() else { return };
    if let Err(e) = nehir_core::platform::set_run_at_login("Nehir", &exe, on) {
        tracing::warn!("otomatik başlatma ayarlanamadı: {e}");
    }
}

#[cfg(unix)]
fn autostart_file() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("autostart/nehir.desktop"))
}

#[cfg(unix)]
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
    !ext.is_empty() && settings.capture_extensions.contains(&ext)
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
                    // Yalnızca durum değişimlerinde (ilerleme bildirimlerinde değil) "bitince" denetimi.
                    let check = !matches!(d.state, nehir_core::model::State::Downloading | nehir_core::model::State::Probing | nehir_core::model::State::Processing);
                    let _ = app.emit("download-updated", d);
                    if check {
                        check_after_all(&app);
                    }
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
                        notify(&app, &d, true, is_turkish(&settings));
                    }
                    app.state::<AppState>().after.lock().unwrap().completed_since = true;
                    let _ = app.emit("download-completed", d);
                    check_after_all(&app);
                }
                Ok(Event::Failed(d)) => {
                    let settings = app.state::<AppState>().downloads.settings();
                    if settings.notifications {
                        notify(&app, &d, false, is_turkish(&settings));
                    }
                    check_after_all(&app);
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    });
}

// ---------- Bildirimler ----------

/// Yanıtı (düğme tıklaması) beklenen bildirim sayısı. Her biri bir iş parçacığı tutar; sınırı aşınca düğmesiz gösterilir.
static WAITING_NOTIFICATIONS: AtomicUsize = AtomicUsize::new(0);
const MAX_WAITING_NOTIFICATIONS: usize = 8;

/// Düğmeli masaüstü bildirimi: tamamlanınca "Aç" / "Klasörü göster", başarısız olunca "Tekrar dene".
/// Bildirimin kendisine tıklamak Nehir'i öne getirir.
fn notify(app: &AppHandle, d: &nehir_core::model::Download, completed: bool, tr: bool) {
    let title = match (completed, tr) {
        (true, true) => "İndirme tamamlandı",
        (true, false) => "Download completed",
        (false, true) => "İndirme başarısız",
        (false, false) => "Download failed",
    };
    let mut body = d.file_name.clone();
    if d.checksum_ok == Some(true) {
        body.push_str(if tr { "\n✓ Dosya doğrulandı" } else { "\n✓ File verified" });
    } else if d.checksum_ok == Some(false) {
        body.push_str(if tr { "\n⚠ Doğrulama başarısız: dosya bozuk olabilir" } else { "\n⚠ Verification failed: the file may be corrupt" });
    }
    let mut n = notify_rust::Notification::new();
    n.appname("Nehir").summary(title).body(&body).icon("nehir");
    let with_actions = WAITING_NOTIFICATIONS.load(Ordering::SeqCst) < MAX_WAITING_NOTIFICATIONS;
    if with_actions {
        n.action("default", if tr { "Nehir'i aç" } else { "Open Nehir" });
        if completed {
            n.action("open", if tr { "Aç" } else { "Open" });
            n.action("folder", if tr { "Klasörü göster" } else { "Show in folder" });
        } else {
            n.action("retry", if tr { "Tekrar dene" } else { "Retry" });
        }
    }
    // Windows: notify-rust düğme/yanıt desteği sunmaz (`show()` bir tutamaç döndürmez); sade bildirim gösterilir.
    #[cfg(windows)]
    {
        let _ = (app, with_actions);
        if let Err(e) = n.show() {
            tracing::warn!("bildirim gösterilemedi: {e}");
        }
    }
    #[cfg(unix)]
    {
        let handle = match n.show() {
            Ok(h) => h,
            Err(e) => {
                tracing::warn!("bildirim gösterilemedi: {e}");
                return;
            }
        };
        if !with_actions {
            return;
        }
        let app = app.clone();
        let (id, path, dir) = (d.id, d.final_path(), d.dir.clone());
        WAITING_NOTIFICATIONS.fetch_add(1, Ordering::SeqCst);
        std::thread::spawn(move || {
            handle.wait_for_action(|action| match action {
                "open" => {
                    let _ = open_path(&path);
                }
                "folder" => {
                    let _ = open_path(&dir);
                }
                "retry" => app.state::<AppState>().downloads.resume(id),
                "default" => {
                    let a = app.clone();
                    let _ = app.run_on_main_thread(move || show_main(&a));
                }
                _ => {}
            });
            WAITING_NOTIFICATIONS.fetch_sub(1, Ordering::SeqCst);
        });
    }
}

// ---------- İndirmeler bitince ----------

/// Kuyrukta iş kalmadıysa ve bir işlem kuruluysa geri sayımı başlatır.
fn check_after_all(app: &AppHandle) {
    use nehir_core::model::State as S;
    let state = app.state::<AppState>();
    let action = {
        let a = state.after.lock().unwrap();
        if a.action.is_empty() || a.action == "none" || !a.completed_since || a.countdown.is_some() {
            return;
        }
        a.action.clone()
    };
    let busy = state.downloads.list().iter().any(|d| matches!(d.state, S::Queued | S::Probing | S::Downloading | S::Processing | S::Waiting));
    if busy {
        return;
    }
    let (tx, rx) = tokio::sync::oneshot::channel::<bool>();
    state.after.lock().unwrap().countdown = Some(tx);
    let _ = app.emit("after-all-countdown", json!({ "action": action, "seconds": AFTER_COUNTDOWN_SECS }));
    let a = app.clone();
    let _ = app.run_on_main_thread(move || show_main(&a));
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let go = tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(AFTER_COUNTDOWN_SECS)) => true,
            r = rx => r.unwrap_or(false),
        };
        let state = app.state::<AppState>();
        // Geri sayım sırasında yeni indirme eklendiyse şimdilik bekle (işlem kurulu kalır, sonra yeniden tetiklenir).
        let busy = state.downloads.list().iter().any(|d| matches!(d.state, S::Queued | S::Probing | S::Downloading | S::Processing | S::Waiting));
        {
            let mut a = state.after.lock().unwrap();
            a.countdown = None;
            if !go || busy {
                if busy {
                    let _ = app.emit("after-all-changed", a.action.clone());
                }
                return;
            }
            a.action = "none".into();
            a.completed_since = false;
        }
        let _ = app.emit("after-all-changed", "none");
        run_after_all(&app, &action);
    });
}

fn run_after_all(app: &AppHandle, action: &str) {
    tracing::info!("indirmeler bitti, işlem: {action}");
    let ok = match action {
        "sleep" => nehir_core::platform::suspend(),
        "shutdown" => nehir_core::platform::power_off(),
        "quit" => {
            app.exit(0);
            true
        }
        _ => true,
    };
    if !ok {
        let _ = app.emit("after-all-failed", action);
    }
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
            app.manage(AppState { downloads, bridge: Mutex::new(None), listening: Arc::new(AtomicBool::new(false)), after: Mutex::new(AfterAll::default()) });
            let handle = app.handle().clone();

            // Kurulu tarayıcılara native messaging kaydı (uygulama kapalıyken eklentinin onu başlatabilmesi için).
            match register_native_hosts() {
                Ok(done) => tracing::info!("native host kaydedildi: {}", done.join(", ")),
                Err(e) => tracing::info!("native host kaydedilmedi: {e}"),
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
            verify_checksum, get_after_all, set_after_all, answer_after_all, open_extensions_page, open_extension_folder,
        ])
        .run(tauri::generate_context!())
        .expect("Nehir başlatılamadı");
}
