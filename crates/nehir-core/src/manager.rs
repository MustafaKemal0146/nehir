//! İndirme yöneticisi: kuyruk, eşzamanlılık, duraklat/devam, kalıcılık ve olay yayını.

use crate::engine::{self, Config, EngineError, Outcome, Progress, SpeedLimit};
use crate::model::*;
use crate::settings::{write_atomic, Paths, Settings};
use crate::util;
use crate::video::{self, VideoOpts};
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const INTENT_NONE: u8 = 0;
const INTENT_PAUSE: u8 = 1;
const INTENT_CANCEL: u8 = 2;
const INTENT_REMOVE: u8 = 3;
const INTENT_REMOVE_FILE: u8 = 4;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", content = "data")]
pub enum Event {
    Updated(Download),
    Removed(Uuid),
    Settings(Settings),
    Completed(Download),
    Failed(Download),
}

struct Running {
    cancel: CancellationToken,
    intent: Arc<AtomicU8>,
}

enum Done {
    Completed(Option<PathBuf>),
    Stopped,
}

/// Başarısız deneme: `transient` ise ağ/sunucu kaynaklıdır, bekleyip otomatik yeniden denenir.
struct Fail {
    msg: String,
    transient: bool,
}

impl From<String> for Fail {
    fn from(msg: String) -> Fail {
        Fail { msg, transient: false }
    }
}
impl From<&str> for Fail {
    fn from(msg: &str) -> Fail {
        Fail { msg: msg.to_string(), transient: false }
    }
}
impl Fail {
    fn from_engine(e: EngineError) -> Fail {
        Fail { transient: e.is_transient(), msg: e.to_string() }
    }
}

/// Yeniden deneme ve bekleme ayarları (testlerde küçültülür).
#[derive(Debug, Clone, Copy)]
pub struct Tuning {
    /// Bir parça için art arda en çok deneme.
    pub max_attempts: u32,
    /// Motorun bekleme sürelerinin çarpanı.
    pub backoff_scale: f64,
    /// Ağ/sunucu hatasından sonraki ilk bekleme (sonrakiler 2 katına çıkar, en çok 2 dk).
    pub retry_unit_ms: u64,
    /// Sunucuya ulaşılamıyorsa kaç ms'de bir tekrar yoklanır.
    pub probe_ms: u64,
    /// Yeniden deneme döngüsünün aralığı.
    pub tick_ms: u64,
}

impl Default for Tuning {
    fn default() -> Self {
        Tuning { max_attempts: engine::DEFAULT_MAX_ATTEMPTS, backoff_scale: 1.0, retry_unit_ms: 5_000, probe_ms: 5_000, tick_ms: 1_000 }
    }
}

/// Art arda en çok kaç kez, ilerleme olmadan otomatik denenir.
const MAX_AUTO_RETRIES: u32 = 40;

struct SpeedSample {
    at: Instant,
    bytes: u64,
    speed: f64,
}

struct Inner {
    items: Mutex<Vec<Download>>,
    tasks: Mutex<HashMap<Uuid, Running>>,
    samples: Mutex<HashMap<Uuid, SpeedSample>>,
    tx: broadcast::Sender<Event>,
    settings: RwLock<Settings>,
    limiter: Arc<SpeedLimit>,
    client: reqwest::Client,
    paths: Paths,
    last_save: Mutex<Instant>,
    extensions_seen: Mutex<HashMap<String, u64>>,
    rt: tokio::runtime::Handle,
    tuning: RwLock<Tuning>,
}

#[derive(Clone)]
pub struct Manager {
    inner: Arc<Inner>,
}

fn public(d: &Download) -> Download {
    let mut c = d.clone();
    c.cookie_header = None;
    c
}

impl Manager {
    /// Tokio çalışma zamanı içinde çağrılmalı.
    pub fn new(paths: Paths) -> Manager {
        let settings = Settings::load(&paths.settings_file());
        let mut items: Vec<Download> = std::fs::read(paths.downloads_file())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        // Uygulama/bilgisayar kapanırken yarıda kalanları kullanıcı durdurmadı; açılışta kendiliğinden sürer.
        // (Kullanıcının bilerek duraklattıkları `Paused` kalır.)
        for d in &mut items {
            if d.state.is_active() {
                d.state = State::Queued;
                d.speed = 0.0;
                d.error = None;
            } else if d.state == State::Waiting {
                d.state = State::Queued; // uygulama yeniden başlayınca hemen tekrar dene
                d.retry_at = None;
            }
        }
        let (tx, _) = broadcast::channel(512);
        let limiter = SpeedLimit::new(settings.speed_limit_kbps * 1024);
        let inner = Arc::new(Inner {
            items: Mutex::new(items),
            tasks: Mutex::new(HashMap::new()),
            samples: Mutex::new(HashMap::new()),
            tx,
            settings: RwLock::new(settings),
            limiter,
            client: engine::build_client().expect("http istemcisi"),
            paths,
            last_save: Mutex::new(Instant::now()),
            extensions_seen: Mutex::new(HashMap::new()),
            rt: tokio::runtime::Handle::current(),
            tuning: RwLock::new(Tuning::default()),
        });
        let m = Manager { inner };
        m.schedule();
        m.spawn_retry_loop();
        m
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.inner.tx.subscribe()
    }
    pub fn paths(&self) -> &Paths {
        &self.inner.paths
    }
    pub fn settings(&self) -> Settings {
        self.inner.settings.read().unwrap().clone()
    }
    pub fn http_client(&self) -> reqwest::Client {
        self.inner.client.clone()
    }

    pub fn update_settings(&self, new: Settings) -> Settings {
        let new = new.sanitized();
        self.inner.limiter.set(new.speed_limit_kbps * 1024);
        *self.inner.settings.write().unwrap() = new.clone();
        if let Err(e) = new.save(&self.inner.paths.settings_file()) {
            tracing::warn!("ayarlar yazılamadı: {e}");
        }
        let _ = self.inner.tx.send(Event::Settings(new.clone()));
        self.schedule();
        new
    }

    pub fn list(&self) -> Vec<Download> {
        self.inner.items.lock().unwrap().iter().map(public).collect()
    }

    pub fn get(&self, id: Uuid) -> Option<Download> {
        self.inner.items.lock().unwrap().iter().find(|d| d.id == id).map(public)
    }

    pub fn note_extension(&self, browser: &str) {
        self.inner.extensions_seen.lock().unwrap().insert(browser.to_string(), util::now_secs());
    }
    pub fn extensions_seen(&self) -> HashMap<String, u64> {
        self.inner.extensions_seen.lock().unwrap().clone()
    }
    pub fn ytdlp_path(&self) -> Option<PathBuf> {
        video::locate_ytdlp(&self.settings().ytdlp_path, &self.inner.paths)
    }

    // ---------- Komutlar ----------

    pub fn add(&self, req: NewDownload) -> Result<Download, String> {
        let url = req.url.trim().to_string();
        let parsed = url::Url::parse(&url).map_err(|_| "geçersiz adres".to_string())?;
        if !matches!(parsed.scheme(), "http" | "https") || url.len() > 8192 {
            return Err("yalnızca http/https adresleri desteklenir".into());
        }
        let settings = self.settings();

        {
            // Aynı adresin art arda gelen kopya isteklerini birleştir.
            let items = self.inner.items.lock().unwrap();
            if let Some(d) = items.iter().find(|d| {
                d.url == url && d.kind == req.kind && matches!(d.state, State::Queued | State::Probing | State::Downloading)
                    && util::now_secs().saturating_sub(d.created_at) < 30
            }) {
                return Ok(public(d));
            }
        }

        let explicit_dir = req.save_dir.is_some();
        let mut dir = req.save_dir.clone().unwrap_or_else(|| settings.download_dir.clone());
        let file_name = req.filename.as_deref().map(util::sanitize_file_name).filter(|n| !n.is_empty());
        let audio = req.kind == Kind::Video && req.quality.as_deref() == Some("audio");
        let (category, name) = match req.kind {
            Kind::Video => {
                let cat = if audio { Category::Music } else { Category::Video };
                if settings.categorize && !explicit_dir {
                    dir = dir.join(cat.folder_name());
                }
                (cat, file_name.unwrap_or_default())
            }
            Kind::File => (
                file_name.as_deref().map(Category::from_file_name).unwrap_or(Category::Other),
                file_name.unwrap_or_default(),
            ),
        };

        let id = Uuid::new_v4();
        if req.kind == Kind::Video && !req.cookies.is_empty() {
            let f = self.inner.paths.cookies_dir().join(format!("{id}.txt"));
            if let Some(parent) = f.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(e) = video::write_cookie_file(&f, &req.cookies) {
                tracing::warn!("çerez dosyası yazılamadı: {e}");
            }
        }

        let d = Download {
            id,
            url,
            file_name: name,
            dir,
            kind: req.kind,
            category,
            state: State::Queued,
            total_bytes: -1,
            received_bytes: 0,
            speed: 0.0,
            segments: Vec::new(),
            segment_count: req.segments.unwrap_or(settings.max_connections).clamp(1, 32),
            supports_ranges: false,
            error: None,
            stage: None,
            created_at: util::now_secs(),
            finished_at: None,
            referer: req.referer.filter(|s| !s.is_empty()),
            user_agent: req.user_agent.filter(|s| !s.is_empty()),
            cookie_header: req.cookie_header.filter(|s| !s.is_empty()),
            extra_headers: req.headers,
            explicit_dir,
            quality: req.quality.filter(|q| q == "audio" || q == "best" || q.chars().all(|c| c.is_ascii_digit())),
            title_hint: req
                .title_hint
                .as_deref()
                .map(util::sanitize_title)
                .map(|t| t.chars().take(150).collect::<String>())
                .filter(|t| !t.trim().is_empty()),
            fallback_urls: req
                .fallback_urls
                .into_iter()
                .filter(|u| url::Url::parse(u).map(|p| matches!(p.scheme(), "http" | "https")).unwrap_or(false))
                .take(8)
                .collect(),
            retries: 0,
            retry_at: None,
            retry_mark: 0,
        };
        let out = public(&d);
        self.inner.items.lock().unwrap().insert(0, d);
        let _ = self.inner.tx.send(Event::Updated(out.clone()));
        self.save_now();
        self.schedule();
        Ok(out)
    }

    pub fn pause(&self, id: Uuid) {
        if !self.signal(id, INTENT_PAUSE) {
            self.update(id, |d| {
                if matches!(d.state, State::Queued | State::Waiting) {
                    d.state = State::Paused;
                    d.retry_at = None;
                }
            });
            self.save_now();
        }
    }

    pub fn resume(&self, id: Uuid) {
        self.update(id, |d| {
            if matches!(d.state, State::Paused | State::Failed) {
                d.state = State::Queued;
                d.error = None;
            }
        });
        self.save_now();
        self.schedule();
    }

    pub fn pause_all(&self) {
        let ids: Vec<Uuid> = self.inner.items.lock().unwrap().iter().filter(|d| matches!(d.state, State::Queued | State::Waiting) || d.state.is_active()).map(|d| d.id).collect();
        for id in ids {
            self.pause(id);
        }
    }

    pub fn resume_all(&self) {
        let ids: Vec<Uuid> = self.inner.items.lock().unwrap().iter().filter(|d| d.state == State::Paused).map(|d| d.id).collect();
        for id in ids {
            self.resume(id);
        }
    }

    /// İptal: kısmi dosya silinir, kayıt listede "İptal edildi" olarak kalır.
    pub fn cancel(&self, id: Uuid) {
        if !self.signal(id, INTENT_CANCEL) {
            let snapshot = self.update(id, |d| {
                if !matches!(d.state, State::Completed | State::Cancelled) {
                    d.state = State::Cancelled;
                    d.speed = 0.0;
                }
                d.clone()
            });
            if let Some(d) = snapshot {
                cleanup_work(&d);
            }
            self.save_now();
            self.schedule();
        }
    }

    /// Listeden kaldırır; `delete_file` ise tamamlanmış dosyayı da siler.
    pub fn remove(&self, id: Uuid, delete_file: bool) {
        if self.signal(id, if delete_file { INTENT_REMOVE_FILE } else { INTENT_REMOVE }) {
            return;
        }
        self.remove_now(id, delete_file);
        self.schedule();
    }

    pub fn clear_finished(&self) {
        let ids: Vec<Uuid> = self
            .inner
            .items
            .lock()
            .unwrap()
            .iter()
            .filter(|d| matches!(d.state, State::Completed | State::Cancelled | State::Failed))
            .map(|d| d.id)
            .collect();
        for id in ids {
            self.remove_now(id, false);
        }
    }

    // ---------- İç işler ----------

    fn signal(&self, id: Uuid, intent: u8) -> bool {
        let tasks = self.inner.tasks.lock().unwrap();
        if let Some(r) = tasks.get(&id) {
            r.intent.store(intent, Ordering::SeqCst);
            r.cancel.cancel();
            true
        } else {
            false
        }
    }

    fn remove_now(&self, id: Uuid, delete_file: bool) {
        let removed = {
            let mut items = self.inner.items.lock().unwrap();
            items.iter().position(|d| d.id == id).map(|i| items.remove(i))
        };
        if let Some(d) = removed {
            cleanup_work(&d);
            let _ = std::fs::remove_file(self.inner.paths.cookies_dir().join(format!("{id}.txt")));
            if delete_file && d.state == State::Completed {
                let _ = std::fs::remove_file(d.final_path());
            }
            let _ = self.inner.tx.send(Event::Removed(id));
        }
        self.save_now();
    }

    fn update<R>(&self, id: Uuid, f: impl FnOnce(&mut Download) -> R) -> Option<R> {
        let (r, snapshot) = {
            let mut items = self.inner.items.lock().unwrap();
            let d = items.iter_mut().find(|d| d.id == id)?;
            let r = f(d);
            (r, public(d))
        };
        let _ = self.inner.tx.send(Event::Updated(snapshot));
        Some(r)
    }

    fn schedule(&self) {
        let max = self.settings().max_concurrent as usize;
        loop {
            let next = {
                let mut items = self.inner.items.lock().unwrap();
                let active = items.iter().filter(|d| d.state.is_active()).count();
                if active >= max {
                    break;
                }
                // Liste yeni → eski sıralı; en eski kuyruktakini başlat.
                match items.iter_mut().rev().find(|d| d.state == State::Queued) {
                    Some(d) => {
                        d.state = State::Probing;
                        d.error = None;
                        Some((d.id, public(d)))
                    }
                    None => None,
                }
            };
            let Some((id, snap)) = next else { break };
            let _ = self.inner.tx.send(Event::Updated(snap));
            let running = Running { cancel: CancellationToken::new(), intent: Arc::new(AtomicU8::new(INTENT_NONE)) };
            let cancel = running.cancel.clone();
            let intent = running.intent.clone();
            self.inner.tasks.lock().unwrap().insert(id, running);
            let m = self.clone();
            self.inner.rt.spawn(async move {
                let result = m.run_one(id, cancel).await;
                m.finish(id, result, intent.load(Ordering::SeqCst));
            });
        }
    }

    fn save_now(&self) {
        *self.inner.last_save.lock().unwrap() = Instant::now();
        let data = serde_json::to_vec(&*self.inner.items.lock().unwrap()).unwrap_or_default();
        if let Err(e) = write_atomic(&self.inner.paths.downloads_file(), &data, 0o600) {
            tracing::warn!("indirme listesi yazılamadı: {e}");
        }
    }

    fn save_if_due(&self) {
        let due = self.inner.last_save.lock().unwrap().elapsed() > Duration::from_secs(3);
        if due {
            self.save_now();
        }
    }

    fn headers_for(d: &Download) -> Vec<(String, String)> {
        let mut h: Vec<(String, String)> = Vec::new();
        h.push((
            "User-Agent".into(),
            d.user_agent.clone().unwrap_or_else(|| "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36".into()),
        ));
        if let Some(r) = &d.referer {
            h.push(("Referer".into(), r.clone()));
        }
        if let Some(c) = &d.cookie_header {
            h.push(("Cookie".into(), c.clone()));
        }
        for (k, v) in &d.extra_headers {
            if !matches!(k.to_ascii_lowercase().as_str(), "host" | "content-length" | "range" | "accept-encoding" | "user-agent" | "referer" | "cookie") {
                h.push((k.clone(), v.clone()));
            }
        }
        h
    }

    async fn run_one(&self, id: Uuid, cancel: CancellationToken) -> Result<Done, Fail> {
        let d = self.get_full(id).ok_or("kayıt bulunamadı")?;
        match d.kind {
            Kind::File => self.run_file(d, cancel).await,
            Kind::Video => self.run_video(d, cancel).await,
        }
    }

    fn get_full(&self, id: Uuid) -> Option<Download> {
        self.inner.items.lock().unwrap().iter().find(|d| d.id == id).cloned()
    }

    async fn run_file(&self, d: Download, cancel: CancellationToken) -> Result<Done, Fail> {
        let id = d.id;
        let headers = Self::headers_for(&d);
        let settings = self.settings();
        let client = self.inner.client.clone();

        let can_resume = d.total_bytes > 0 && !d.segments.is_empty() && d.part_path().exists() && !d.file_name.is_empty();
        let (segments, total, resume) = if can_resume {
            (d.segments.clone(), Some(d.total_bytes as u64), true)
        } else {
            let info = tokio::select! {
                r = engine::probe(&client, &d.url, &headers) => r.map_err(Fail::from_engine)?,
                _ = cancel.cancelled() => return Ok(Done::Stopped),
            };
            let mut name = if !d.file_name.is_empty() {
                d.file_name.clone()
            } else if let Some(hint) = d.title_hint.clone() {
                // Algılanan medya: sayfa başlığı + gerçek uzantı (850x480.mp4 yerine "Video Adı.mp4").
                let ext = info
                    .file_name
                    .as_deref()
                    .and_then(ext_of)
                    .or_else(|| util::file_name_from_url(&d.url).as_deref().and_then(ext_of))
                    .or_else(|| info.content_type.as_deref().and_then(ext_for_mime).map(String::from))
                    .unwrap_or_else(|| "mp4".into());
                format!("{hint}.{ext}")
            } else {
                info.file_name
                    .clone()
                    .or_else(|| util::file_name_from_url(&d.url))
                    .unwrap_or_else(|| "indirme".into())
            };
            if !name.contains('.') {
                if let Some(ext) = info.content_type.as_deref().and_then(ext_for_mime) {
                    name = format!("{name}.{ext}");
                }
            }
            let category = Category::from_file_name(&name);
            let mut dir = d.dir.clone();
            if settings.categorize && !d.explicit_dir && category != Category::Other && d.dir == settings.download_dir {
                dir = dir.join(category.folder_name());
            }
            tokio::fs::create_dir_all(&dir).await.map_err(|e| format!("klasör oluşturulamadı: {e}"))?;
            let name = util::unique_file_name(&dir, &name);
            // Aynı anda gelen başka bir indirme aynı adı almasın.
            let work = dir.join(WORK_DIR_NAME);
            let _ = std::fs::create_dir_all(&work);
            let _ = std::fs::OpenOptions::new().write(true).create(true).truncate(true).open(work.join(format!("{name}.nehirpart")));

            let segments = if info.ranges {
                engine::plan_segments(info.total.unwrap_or(0), d.segment_count, 512 * 1024)
            } else {
                Vec::new()
            };
            let total_i = info.total.map(|t| t as i64).unwrap_or(-1);
            let (n2, d2, s2) = (name.clone(), dir.clone(), segments.clone());
            self.update(id, move |it| {
                it.file_name = n2;
                it.dir = d2;
                it.category = category;
                it.total_bytes = total_i;
                it.supports_ranges = info.ranges;
                it.segments = s2;
                it.received_bytes = 0;
            });
            self.save_now();
            (segments, info.total, false)
        };

        self.update(id, |it| it.state = State::Downloading);
        let d = self.get_full(id).ok_or("kayıt bulunamadı")?;
        let cfg = Config {
            url: d.url.clone(),
            headers,
            part_path: d.part_path(),
            max_connections: d.segment_count,
            min_segment_size: 512 * 1024,
            segments,
            total,
            resume,
            limiter: self.inner.limiter.clone(),
            max_attempts: self.tuning().max_attempts,
            backoff_scale: self.tuning().backoff_scale,
        };
        let m = self.clone();
        let outcome = engine::run(cfg, client, cancel, move |p| m.on_progress(id, p)).await;
        match outcome {
            Ok(Outcome::Completed) => {
                let d = self.get_full(id).ok_or("kayıt bulunamadı")?;
                let mut target = d.final_path();
                if target.exists() {
                    target = d.dir.join(util::unique_file_name(&d.dir, &d.file_name));
                }
                tokio::fs::rename(d.part_path(), &target).await.map_err(|e| format!("dosya taşınamadı: {e}"))?;
                let _ = std::fs::remove_dir(d.work_dir());
                Ok(Done::Completed(Some(target)))
            }
            Ok(Outcome::Stopped) => Ok(Done::Stopped),
            Err(e) => Err(Fail::from_engine(e)),
        }
    }

    fn on_progress(&self, id: Uuid, p: Progress) {
        let speed = {
            let mut samples = self.inner.samples.lock().unwrap();
            let now = Instant::now();
            let s = samples.entry(id).or_insert(SpeedSample { at: now, bytes: p.received, speed: 0.0 });
            let dt = now.duration_since(s.at).as_secs_f64();
            if dt >= 0.2 {
                let inst = p.received.saturating_sub(s.bytes) as f64 / dt;
                s.speed = if s.speed == 0.0 { inst } else { 0.7 * s.speed + 0.3 * inst };
                s.at = now;
                s.bytes = p.received;
            }
            s.speed
        };
        self.update(id, |d| {
            d.received_bytes = p.received;
            if !p.segments.is_empty() {
                d.segments = p.segments;
            }
            d.speed = speed;
        });
        self.save_if_due();
    }

    async fn run_video(&self, d: Download, cancel: CancellationToken) -> Result<Done, Fail> {
        let id = d.id;
        let settings = self.settings();
        let ytdlp = video::locate_ytdlp(&settings.ytdlp_path, &self.inner.paths).ok_or_else(|| "ytdlp_missing".to_string())?;
        let cookie_file = Some(self.inner.paths.cookies_dir().join(format!("{id}.txt"))).filter(|p| p.exists());
        self.update(id, |it| it.state = State::Downloading);

        let last = Mutex::new(Instant::now() - Duration::from_secs(1));
        let (m1, m2) = (self.clone(), self.clone());
        // Akış/medya adresinde (m3u8, mpd, 850x480.mp4) yt-dlp iyi bir başlık bulamaz: sayfa başlığını kullan.
        let stream = regex::Regex::new(r"(?i)\.(m3u8|mpd|mp4|webm|mkv|mov|m4v)(\?|#|$)").unwrap().is_match(&d.url);
        let name_stem = d.title_hint.clone().filter(|_| stream).map(|h| {
            let ext = if d.audio_only() { "mp3" } else { "mp4" };
            let unique = util::unique_file_name(&d.dir, &format!("{h}.{ext}"));
            unique.strip_suffix(&format!(".{ext}")).unwrap_or(&unique).to_string()
        });
        if let Some(stem) = &name_stem {
            let shown = stem.clone();
            self.update(id, |it| it.file_name = shown);
        }
        let opts = VideoOpts {
            work_dir: d.work_dir().join(id.to_string()),
            name_stem,
            url: d.url.clone(),
            out_dir: d.dir.clone(),
            quality: d.quality.clone().unwrap_or_else(|| settings.video_quality.clone()),
            user_agent: d.user_agent.clone(),
            referer: d.referer.clone(),
            cookie_file,
            ytdlp,
            connections: 4,
        };
        let res = video::run(
            opts,
            &self.inner.paths,
            cancel,
            move |p| {
                let mut l = last.lock().unwrap();
                if l.elapsed() < Duration::from_millis(250) {
                    return;
                }
                *l = Instant::now();
                drop(l);
                m1.update(id, |it| {
                    it.received_bytes = p.downloaded;
                    it.total_bytes = p.total;
                    it.speed = p.speed;
                });
            },
            move |stage| {
                if let Some(title) = stage.strip_prefix("title:") {
                    let name = util::clean_video_name(&format!("{}.mp4", util::sanitize_title(title)));
                    let name = name.strip_suffix(".mp4").unwrap_or(&name).to_string();
                    m2.update(id, |it| if it.title_hint.is_none() || it.file_name.is_empty() { it.file_name = name });
                    return;
                }
                m2.update(id, |it| {
                    it.state = if stage == "merging" || stage == "converting" || stage == "extracting_audio" { State::Processing } else { State::Downloading };
                    it.stage = Some(stage);
                });
            },
        )
        .await
        .map_err(|msg| Fail { transient: video::is_transient_error(&msg), msg })?;
        match res {
            Some(path) => Ok(Done::Completed(Some(path))),
            None => Ok(Done::Stopped),
        }
    }

    fn finish(&self, id: Uuid, result: Result<Done, Fail>, intent: u8) {
        self.inner.tasks.lock().unwrap().remove(&id);
        self.inner.samples.lock().unwrap().remove(&id);
        match result {
            Ok(Done::Completed(path)) => {
                let _ = std::fs::remove_file(self.inner.paths.cookies_dir().join(format!("{id}.txt")));
                let is_video = self.get_full(id).map(|d| d.kind == Kind::Video).unwrap_or(false);
                let path = if is_video { path.map(tidy_video_file) } else { path };
                if let Some(d) = self.get_full(id) {
                    if is_video {
                        cleanup_work(&d);
                    }
                }
                self.update(id, |d| {
                    if let Some(p) = &path {
                        if let Some(n) = p.file_name() {
                            d.file_name = n.to_string_lossy().into_owned();
                        }
                        if let Some(dir) = p.parent() {
                            d.dir = dir.to_path_buf();
                        }
                        d.category = Category::from_file_name(&d.file_name);
                        if d.kind == Kind::Video {
                            d.category = if d.audio_only() { Category::Music } else { Category::Video };
                            if let Ok(meta) = std::fs::metadata(p) {
                                d.total_bytes = meta.len() as i64;
                            }
                        }
                    }
                    d.state = State::Completed;
                    d.received_bytes = d.total_bytes.max(0) as u64;
                    d.speed = 0.0;
                    d.segments.clear();
                    d.cookie_header = None;
                    d.stage = None;
                    d.error = None;
                    d.finished_at = Some(util::now_secs());
                });
                if let Some(d) = self.get(id) {
                    let _ = self.inner.tx.send(Event::Completed(d));
                }
            }
            Ok(Done::Stopped) => match intent {
                INTENT_CANCEL => {
                    let snapshot = self.update(id, |d| {
                        d.state = State::Cancelled;
                        d.speed = 0.0;
                        d.clone()
                    });
                    if let Some(d) = snapshot {
                        cleanup_work(&d);
                    }
                }
                INTENT_REMOVE | INTENT_REMOVE_FILE => self.remove_now(id, intent == INTENT_REMOVE_FILE),
                _ => {
                    self.update(id, |d| {
                        d.state = State::Paused;
                        d.speed = 0.0;
                    });
                }
            },
            Err(fail) => {
                let msg = fail.msg.clone();
                if intent == INTENT_REMOVE || intent == INTENT_REMOVE_FILE {
                    self.remove_now(id, false);
                } else if fail.transient && self.schedule_retry(id, &msg) {
                    tracing::info!("geçici hata, otomatik yeniden denenecek: {msg}");
                } else if self.try_fallback(id) {
                    tracing::info!("yedek adres deneniyor ({msg})");
                } else {
                    self.update(id, |d| {
                        d.state = State::Failed;
                        d.error = Some(msg);
                        d.speed = 0.0;
                    });
                    if let Some(d) = self.get(id) {
                        let _ = self.inner.tx.send(Event::Failed(d));
                    }
                }
            }
        }
        self.save_now();
        self.schedule();
    }
}

impl Manager {
    fn tuning(&self) -> Tuning {
        *self.inner.tuning.read().unwrap()
    }

    /// Yeniden deneme/bekleme sürelerini değiştirir (testler için).
    #[doc(hidden)]
    pub fn set_tuning(&self, t: Tuning) {
        *self.inner.tuning.write().unwrap() = t;
    }

    /// Ağ/sunucu hatasında indirmeyi "bekliyor" durumuna alır; `false` ise vazgeçilmiştir (çok deneme, ilerleme yok).
    fn schedule_retry(&self, id: Uuid, msg: &str) -> bool {
        let t = self.tuning();
        let now = util::now_millis();
        self.update(id, |d| {
            if d.received_bytes > d.retry_mark {
                d.retries = 0; // ilerleme oldu: sayaç sıfırlanır
                d.retry_mark = d.received_bytes;
            }
            if d.retries >= MAX_AUTO_RETRIES {
                return false;
            }
            d.retries += 1;
            let delay = (t.retry_unit_ms << (d.retries - 1).min(5)).min(120_000.max(t.retry_unit_ms));
            d.state = State::Waiting;
            d.error = Some(msg.to_string());
            d.speed = 0.0;
            d.retry_at = Some(now + delay);
            true
        })
        .unwrap_or(false)
    }

    /// Bekleyen indirmeleri izler: sunucuya yeniden ulaşılabilir olunca ya da bilgisayar uykudan uyanınca sürdürür.
    fn spawn_retry_loop(&self) {
        let weak = Arc::downgrade(&self.inner);
        let tick = Duration::from_millis(self.tuning().tick_ms);
        self.inner.rt.spawn(async move {
            let mut last_wall = std::time::SystemTime::now();
            let mut last_mono = Instant::now();
            loop {
                tokio::time::sleep(tick).await;
                let Some(inner) = weak.upgrade() else { break };
                let m = Manager { inner };
                // Uykudan uyanma: duvar saati, tek yönlü (uykuda durmayan) saatten çok ilerlemiş demektir.
                let wall = std::time::SystemTime::now();
                let wall_dt = wall.duration_since(last_wall).unwrap_or_default();
                let mono_dt = last_mono.elapsed();
                last_wall = wall;
                last_mono = Instant::now();
                let woke = wall_dt > mono_dt + Duration::from_secs(8);
                m.retry_tick(woke).await;
            }
        });
    }

    async fn retry_tick(&self, woke: bool) {
        let waiting: Vec<(Uuid, String, Option<u64>)> = self
            .inner
            .items
            .lock()
            .unwrap()
            .iter()
            .filter(|d| d.state == State::Waiting)
            .map(|d| (d.id, d.url.clone(), d.retry_at))
            .collect();
        if waiting.is_empty() {
            return;
        }
        let probe_ms = self.tuning().probe_ms;
        let mut resumed = false;
        for (id, url, retry_at) in waiting {
            let now = util::now_millis();
            if !(woke || retry_at.map(|t| now >= t).unwrap_or(true)) {
                continue;
            }
            if reachable(&url).await {
                self.update(id, |d| {
                    if d.state == State::Waiting {
                        d.state = State::Queued;
                        d.retry_at = None;
                    }
                });
                resumed = true;
            } else {
                self.update(id, |d| d.retry_at = Some(util::now_millis() + probe_ms));
            }
        }
        if resumed {
            self.save_now();
            self.schedule();
        }
    }

    /// Sayfa adresi indirilemediyse bir sonraki yedek medya adresine geçer (kuyruğa geri koyar).
    fn try_fallback(&self, id: Uuid) -> bool {
        let stream = regex::Regex::new(r"(?i)\.(m3u8|mpd)(\?|#|$)").unwrap();
        self.update(id, |d| {
            if d.fallback_urls.is_empty() {
                return false;
            }
            let next = d.fallback_urls.remove(0);
            let host = |u: &str| url::Url::parse(u).ok().and_then(|p| p.host_str().map(str::to_ascii_lowercase));
            if host(&next) != host(&d.url) {
                d.cookie_header = None; // sayfa çerezleri başka bir sunucuya gönderilmesin
            }
            // Müzik isteği (MP3'e çevirme) her zaman yt-dlp üzerinden yapılır.
            d.kind = if stream.is_match(&next) || d.audio_only() { Kind::Video } else { Kind::File };
            d.url = next;
            d.file_name.clear();
            d.state = State::Queued;
            d.error = None;
            d.stage = None;
            d.total_bytes = -1;
            d.received_bytes = 0;
            d.segments.clear();
            true
        })
        .unwrap_or(false)
    }
}

/// Adresin sunucusuna TCP ile ulaşılabiliyor mu? (Ağın geri gelip gelmediğini anlamak için; üçüncü bir sunucuya gitmez.)
async fn reachable(url: &str) -> bool {
    let Ok(u) = url::Url::parse(url) else { return false };
    let (Some(host), Some(port)) = (u.host_str(), u.port_or_known_default()) else { return false };
    matches!(tokio::time::timeout(Duration::from_secs(3), tokio::net::TcpStream::connect((host, port))).await, Ok(Ok(_)))
}

/// Dosya adının uzantısı (en çok 5 harf/rakam), küçük harfle.
fn ext_of(name: &str) -> Option<String> {
    let (_, e) = name.rsplit_once('.')?;
    (!e.is_empty() && e.len() <= 5 && e.chars().all(|c| c.is_ascii_alphanumeric())).then(|| e.to_ascii_lowercase())
}

/// Yarım/ara dosyaları ve boşalan çalışma klasörünü siler.
fn cleanup_work(d: &Download) {
    let _ = std::fs::remove_file(d.part_path());
    let _ = std::fs::remove_dir_all(d.work_dir().join(d.id.to_string()));
    let _ = std::fs::remove_dir(d.work_dir()); // yalnızca boşsa silinir
}

/// yt-dlp'nin ürettiği adı okunur hale getirir (` [id]` sonekini atar, tam genişlikli karakterleri sadeleştirir).
fn tidy_video_file(path: PathBuf) -> PathBuf {
    let (Some(dir), Some(name)) = (path.parent(), path.file_name().map(|n| n.to_string_lossy().into_owned())) else { return path };
    let clean = util::clean_video_name(&name);
    if clean == name {
        return path;
    }
    let target = dir.join(util::unique_file_name(dir, &clean));
    match std::fs::rename(&path, &target) {
        Ok(()) => target,
        Err(_) => path,
    }
}

fn ext_for_mime(mime: &str) -> Option<&'static str> {
    Some(match mime {
        "application/pdf" => "pdf",
        "application/zip" => "zip",
        "application/x-7z-compressed" => "7z",
        "application/gzip" | "application/x-gzip" => "gz",
        "application/x-iso9660-image" => "iso",
        "application/vnd.debian.binary-package" | "application/x-debian-package" => "deb",
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        "video/x-matroska" => "mkv",
        "audio/mpeg" => "mp3",
        "audio/mp4" => "m4a",
        "audio/flac" => "flac",
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/webp" => "webp",
        _ => return None,
    })
}
