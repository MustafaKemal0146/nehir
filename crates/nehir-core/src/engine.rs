//! Çok bağlantılı (HTTP Range) indirme motoru.
//!
//! Tasarım:
//! * Dosya parçalara bölünür; her parça `Segment { start, end, received }` ile izlenir.
//! * `segment_count` kadar işçi, ortak bir kuyruktan parça çeker. Kuyruk boşalınca boşta kalan
//!   işçi, en çok baytı kalan parçayı ikiye bölüp yarısını devralır (kuyruk sonu hızlanır).
//! * Sunucu 429/503 verirse işçi sayısı düşer, bekleme artar. Range yok sayılırsa tek akışa geçilir.
//! * Parça durumu her ilerleme bildiriminde dışarı verilir; çağıran bunu diske yazarak devam etmeyi sağlar.

use crate::model::Segment;
use futures_util::StreamExt;
use reqwest::{header, Client, StatusCode};
use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::os::unix::fs::FileExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

const FLUSH_BYTES: usize = 256 * 1024;
pub const DEFAULT_MAX_ATTEMPTS: u32 = 12;
const STALL_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("Sunucu HTTP {0} döndürdü")]
    Http(u16),
    /// Bağlantı koptu / zaman aşımı: ağ geri gelince devam edilebilir.
    #[error("{0}")]
    Interrupted(String),
    #[error("Ağ hatası: {0}")]
    Request(#[from] reqwest::Error),
    #[error("Disk hatası: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Msg(String),
}

#[derive(Debug, Clone, Default)]
pub struct ProbeInfo {
    pub total: Option<u64>,
    pub ranges: bool,
    pub file_name: Option<String>,
    pub content_type: Option<String>,
}

/// Bant genişliği sınırlayıcı (tüm indirmeler arasında paylaşılır). 0 = sınırsız.
#[derive(Debug)]
pub struct SpeedLimit {
    bytes_per_sec: AtomicU64,
    next: Mutex<Instant>,
}

impl SpeedLimit {
    pub fn new(bytes_per_sec: u64) -> Arc<Self> {
        Arc::new(Self { bytes_per_sec: AtomicU64::new(bytes_per_sec), next: Mutex::new(Instant::now()) })
    }
    pub fn set(&self, bytes_per_sec: u64) {
        self.bytes_per_sec.store(bytes_per_sec, Ordering::Relaxed);
    }
    async fn acquire(&self, n: usize) {
        let limit = self.bytes_per_sec.load(Ordering::Relaxed);
        if limit == 0 {
            return;
        }
        let wait = {
            let mut next = self.next.lock().unwrap();
            let now = Instant::now();
            let start = (*next).max(now);
            *next = start + Duration::from_secs_f64(n as f64 / limit as f64);
            start.saturating_duration_since(now)
        };
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
    }
}

impl EngineError {
    /// Ağ/sunucu kaynaklı, sonra tekrar denenirse düzelebilecek hata mı? (Disk hatası, 403/404 gibi kalıcı hatalar değil.)
    pub fn is_transient(&self) -> bool {
        match self {
            EngineError::Http(c) => matches!(*c, 408 | 425 | 429 | 500..=599),
            EngineError::Interrupted(_) => true,
            EngineError::Request(e) => !(e.is_builder() || e.is_redirect()),
            EngineError::Io(_) | EngineError::Msg(_) => false,
        }
    }
}

pub struct Config {
    pub url: String,
    /// Cookie, Referer, User-Agent vb. (Accept-Encoding ve Range motor tarafından eklenir).
    pub headers: Vec<(String, String)>,
    pub part_path: PathBuf,
    /// Aynı anda açık bağlantı üst sınırı.
    pub max_connections: u32,
    pub min_segment_size: u64,
    /// Başlangıç planı. Boşsa Range kullanılmadan tek akışla indirilir.
    pub segments: Vec<Segment>,
    pub total: Option<u64>,
    /// `true`: parça dosyası zaten var, devam et. `false`: yeni dosya oluştur ve boyutunu ayır.
    pub resume: bool,
    pub limiter: Arc<SpeedLimit>,
    /// Bir parça için art arda en çok kaç deneme yapılacağı (varsayılan 12).
    pub max_attempts: u32,
    /// Bekleme sürelerinin çarpanı (1.0 normal; testlerde küçültülür).
    pub backoff_scale: f64,
}

#[derive(Debug, Clone)]
pub struct Progress {
    pub received: u64,
    pub segments: Vec<Segment>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Completed,
    /// Duraklatma ya da iptal: son durum `on_progress` ile bildirildi.
    Stopped,
}

/// HTTP istemcisi. Önemli: **yalnızca HTTP/1.1**. HTTP/2 tüm `Range` isteklerini tek bir TCP bağlantısında
/// çoğullar; bu da çok bağlantılı indirmenin amacını (her bağlantı kendi TCP penceresiyle ve sunucunun
/// bağlantı başına sınırlamasından bağımsız hız alır) ortadan kaldırır.
pub fn build_client() -> Result<Client, reqwest::Error> {
    Client::builder()
        .http1_only()
        .connect_timeout(Duration::from_secs(20))
        .pool_max_idle_per_host(32)
        // Bağlantı yeniden kullanımı: parça biter bitmez aynı TCP bağlantısı yeni parça için kullanılır.
        .pool_idle_timeout(Duration::from_secs(60))
        .tcp_nodelay(true)
        .tcp_keepalive(Some(Duration::from_secs(30)))
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
}

fn apply_headers(mut rb: reqwest::RequestBuilder, headers: &[(String, String)]) -> reqwest::RequestBuilder {
    for (k, v) in headers {
        if k.eq_ignore_ascii_case("range") || k.eq_ignore_ascii_case("accept-encoding") {
            continue;
        }
        rb = rb.header(k.as_str(), v.as_str());
    }
    // Sıkıştırılmış yanıt Content-Length / Range hesabını bozar.
    rb.header(header::ACCEPT_ENCODING, "identity")
}

/// `bytes=0-0` ile boyutu ve Range desteğini öğrenir.
pub async fn probe(client: &Client, url: &str, headers: &[(String, String)]) -> Result<ProbeInfo, EngineError> {
    let resp = apply_headers(client.get(url), headers).header(header::RANGE, "bytes=0-0").send().await?;
    let status = resp.status();
    if !status.is_success() {
        return Err(EngineError::Http(status.as_u16()));
    }
    let h = resp.headers();
    let ctype = h
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(';').next().unwrap_or("").trim().to_ascii_lowercase());
    let file_name = h
        .get(header::CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
        .and_then(crate::util::file_name_from_content_disposition);

    let (total, ranges) = if status == StatusCode::PARTIAL_CONTENT {
        let total = h
            .get(header::CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.rsplit('/').next())
            .and_then(|s| s.trim().parse::<u64>().ok());
        (total, total.is_some())
    } else {
        let total = h
            .get(header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|n| *n > 0);
        let accepts = h
            .get(header::ACCEPT_RANGES)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_ascii_lowercase().contains("bytes"))
            .unwrap_or(false);
        (total, accepts && total.is_some())
    };
    drop(resp);
    Ok(ProbeInfo { total, ranges, file_name, content_type: ctype })
}

/// Toplam boyutu `count` parçaya böler (son parça artığı alır).
pub fn plan_segments(total: u64, max_connections: u32, min_segment_size: u64) -> Vec<Segment> {
    let by_size = (total / min_segment_size.max(1)).max(1);
    let count = (max_connections.max(1) as u64).min(by_size);
    let size = total / count;
    (0..count)
        .map(|i| {
            let start = i * size;
            let end = if i == count - 1 { total - 1 } else { start + size - 1 };
            Segment { start, end, received: 0 }
        })
        .collect()
}

struct Shared {
    client: Client,
    cfg_url: String,
    headers: Vec<(String, String)>,
    file: Arc<File>,
    limiter: Arc<SpeedLimit>,
    max_attempts: u32,
    backoff_scale: f64,
    min_split: u64,
    segs: Mutex<Vec<Segment>>,
    queue: Mutex<VecDeque<usize>>,
    alive: AtomicUsize,
    fatal: Mutex<Option<EngineError>>,
    no_ranges: Mutex<bool>,
    abort: CancellationToken,
}

enum SegError {
    /// Sunucu sınırlaması: işçi sayısını düşür.
    Throttle(u16),
    /// Geçici hata: aynı parçayı tekrar dene.
    Retry(EngineError),
    NoRanges,
    Fatal(EngineError),
    Stopped,
}

pub async fn run<F>(cfg: Config, client: Client, cancel: CancellationToken, on_progress: F) -> Result<Outcome, EngineError>
where
    F: Fn(Progress) + Send + Sync + 'static,
{
    if let Some(dir) = cfg.part_path.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    let file = if cfg.resume && cfg.part_path.exists() {
        OpenOptions::new().write(true).open(&cfg.part_path)?
    } else {
        let f = OpenOptions::new().write(true).create(true).truncate(true).open(&cfg.part_path)?;
        if let (Some(total), false) = (cfg.total, cfg.segments.is_empty()) {
            f.set_len(total)?;
        }
        f
    };
    let file = Arc::new(file);
    let on_progress: Arc<dyn Fn(Progress) + Send + Sync> = Arc::new(on_progress);

    if cfg.segments.is_empty() {
        return run_single(&cfg, client, file, cancel, on_progress).await;
    }

    let shared = Arc::new(Shared {
        client: client.clone(),
        cfg_url: cfg.url.clone(),
        headers: cfg.headers.clone(),
        file: file.clone(),
        limiter: cfg.limiter.clone(),
        max_attempts: cfg.max_attempts.max(1),
        backoff_scale: cfg.backoff_scale,
        min_split: cfg.min_segment_size.max(64 * 1024),
        queue: Mutex::new(cfg.segments.iter().enumerate().filter(|(_, s)| !s.is_done()).map(|(i, _)| i).collect()),
        segs: Mutex::new(cfg.segments.clone()),
        alive: AtomicUsize::new(0),
        fatal: Mutex::new(None),
        no_ranges: Mutex::new(false),
        abort: cancel.child_token(),
    });

    let pending = shared.queue.lock().unwrap().len();
    let workers = (cfg.max_connections.max(1) as usize).min(pending.max(1));
    shared.alive.store(workers, Ordering::SeqCst);

    let mut handles = Vec::new();
    for _ in 0..workers {
        handles.push(tokio::spawn(worker(shared.clone())));
    }

    // İlerleme bildirici
    let ticker = {
        let shared = shared.clone();
        let on_progress = on_progress.clone();
        let done = CancellationToken::new();
        let done2 = done.clone();
        let h = tokio::spawn(async move {
            let mut iv = tokio::time::interval(Duration::from_millis(250));
            loop {
                tokio::select! {
                    _ = iv.tick() => on_progress(snapshot(&shared)),
                    _ = done2.cancelled() => break,
                }
            }
        });
        (h, done)
    };

    for h in handles {
        let _ = h.await;
    }
    ticker.1.cancel();
    let _ = ticker.0.await;

    let final_snapshot = snapshot(&shared);
    on_progress(final_snapshot.clone());

    if *shared.no_ranges.lock().unwrap() {
        // Sunucu Range'i yok sayıyor: baştan tek akışla indir.
        drop(file);
        let file = Arc::new(OpenOptions::new().write(true).create(true).truncate(true).open(&cfg.part_path)?);
        return run_single(&cfg, client, file, cancel, on_progress).await;
    }
    if let Some(e) = shared.fatal.lock().unwrap().take() {
        return Err(e);
    }
    if final_snapshot.segments.iter().all(|s| s.is_done()) {
        let f = file.clone();
        tokio::task::spawn_blocking(move || f.sync_data()).await.ok();
        return Ok(Outcome::Completed);
    }
    if cancel.is_cancelled() {
        return Ok(Outcome::Stopped);
    }
    Err(EngineError::Msg("İndirme tamamlanamadı".into()))
}

fn snapshot(shared: &Shared) -> Progress {
    let segments = shared.segs.lock().unwrap().clone();
    let received = segments.iter().map(|s| s.received.min(s.len())).sum();
    Progress { received, segments }
}

async fn worker(shared: Arc<Shared>) {
    let mut attempts: u32 = 0;
    loop {
        if shared.abort.is_cancelled() {
            break;
        }
        let Some(idx) = next_task(&shared) else { break };
        match run_segment(&shared, idx).await {
            Ok(()) => attempts = 0,
            Err(SegError::Stopped) => break,
            Err(SegError::NoRanges) => {
                *shared.no_ranges.lock().unwrap() = true;
                shared.abort.cancel();
                break;
            }
            Err(SegError::Fatal(e)) => {
                set_fatal(&shared, e);
                break;
            }
            Err(SegError::Throttle(status)) => {
                attempts += 1;
                shared.queue.lock().unwrap().push_front(idx);
                if attempts > shared.max_attempts {
                    set_fatal(&shared, EngineError::Http(status));
                    break;
                }
                // Birden fazla işçi varsa biri emekli olur: bağlantı sayısı düşer.
                if shared.alive.load(Ordering::SeqCst) > 1 {
                    shared.alive.fetch_sub(1, Ordering::SeqCst);
                    return;
                }
                sleep_or_abort(&shared, Duration::from_secs_f64((1.5 * attempts as f64).min(15.0) * shared.backoff_scale)).await;
            }
            Err(SegError::Retry(e)) => {
                attempts += 1;
                shared.queue.lock().unwrap().push_front(idx);
                if attempts > shared.max_attempts {
                    set_fatal(&shared, e);
                    break;
                }
                sleep_or_abort(&shared, Duration::from_secs_f64((attempts as f64).min(10.0) * shared.backoff_scale)).await;
            }
        }
    }
    shared.alive.fetch_sub(1, Ordering::SeqCst);
}

fn set_fatal(shared: &Shared, e: EngineError) {
    let mut f = shared.fatal.lock().unwrap();
    if f.is_none() {
        *f = Some(e);
    }
    shared.abort.cancel();
}

async fn sleep_or_abort(shared: &Shared, d: Duration) {
    tokio::select! {
        _ = tokio::time::sleep(d) => {},
        _ = shared.abort.cancelled() => {},
    }
}

/// Kuyruktan parça al; kuyruk boşsa en büyük kalanı ikiye bölüp yarısını devral.
fn next_task(shared: &Shared) -> Option<usize> {
    if let Some(i) = shared.queue.lock().unwrap().pop_front() {
        return Some(i);
    }
    let mut segs = shared.segs.lock().unwrap();
    let (victim, remaining) = segs
        .iter()
        .enumerate()
        .filter(|(_, s)| !s.is_done())
        .map(|(i, s)| (i, s.remaining()))
        .max_by_key(|(_, r)| *r)?;
    if remaining < shared.min_split * 2 {
        return None;
    }
    let v = segs[victim];
    let mid = v.cursor() + remaining / 2;
    segs[victim].end = mid - 1;
    segs.push(Segment { start: mid, end: v.end, received: 0 });
    Some(segs.len() - 1)
}

async fn run_segment(shared: &Shared, idx: usize) -> Result<(), SegError> {
    let (cursor, end) = {
        let segs = shared.segs.lock().unwrap();
        let s = segs[idx];
        if s.is_done() {
            return Ok(());
        }
        (s.cursor(), s.end)
    };
    let rb = apply_headers(shared.client.get(&shared.cfg_url), &shared.headers)
        .header(header::RANGE, format!("bytes={cursor}-{end}"));
    let resp = tokio::select! {
        r = rb.send() => r.map_err(|e| SegError::Retry(e.into()))?,
        _ = shared.abort.cancelled() => return Err(SegError::Stopped),
    };
    match resp.status().as_u16() {
        206 => {}
        200 => return Err(SegError::NoRanges),
        408 | 425 | 429 | 500 | 502 | 503 | 504 => return Err(SegError::Throttle(resp.status().as_u16())),
        c => return Err(SegError::Fatal(EngineError::Http(c))),
    }

    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::with_capacity(FLUSH_BYTES * 2);
    loop {
        let next = tokio::select! {
            n = tokio::time::timeout(STALL_TIMEOUT, stream.next()) => n,
            _ = shared.abort.cancelled() => {
                flush(shared, idx, &mut buf).await.map_err(SegError::Fatal)?;
                return Err(SegError::Stopped);
            }
        };
        match next {
            Err(_) => {
                flush(shared, idx, &mut buf).await.map_err(SegError::Fatal)?;
                return Err(SegError::Retry(EngineError::Interrupted("Bağlantı zaman aşımına uğradı".into())));
            }
            Ok(None) => break,
            Ok(Some(Err(e))) => {
                flush(shared, idx, &mut buf).await.map_err(SegError::Fatal)?;
                return Err(SegError::Retry(e.into()));
            }
            Ok(Some(Ok(chunk))) => {
                shared.limiter.acquire(chunk.len()).await;
                buf.extend_from_slice(&chunk);
                if buf.len() >= FLUSH_BYTES && flush(shared, idx, &mut buf).await.map_err(SegError::Fatal)? {
                    return Ok(()); // parça (bölünme sonrası kısalmış olabilir) tamamlandı
                }
            }
        }
    }
    flush(shared, idx, &mut buf).await.map_err(SegError::Fatal)?;
    let done = shared.segs.lock().unwrap()[idx].is_done();
    if done {
        Ok(())
    } else {
        Err(SegError::Retry(EngineError::Interrupted("Bağlantı erken kapandı".into())))
    }
}

/// Tamponu diske yazar. Parça tamamlandıysa `true` döner. Parça sonrasında bölünmüş olabilir,
/// bu yüzden yazılacak miktar kilit altında sınırlanır ve `received` yazmadan önce artırılır
/// (başka bir işçi aynı baytları devralmasın).
async fn flush(shared: &Shared, idx: usize, buf: &mut Vec<u8>) -> Result<bool, EngineError> {
    if buf.is_empty() {
        return Ok(shared.segs.lock().unwrap()[idx].is_done());
    }
    let (n, offset, done) = {
        let mut segs = shared.segs.lock().unwrap();
        let s = &mut segs[idx];
        let n = (buf.len() as u64).min(s.remaining());
        let offset = s.cursor();
        s.received += n;
        (n as usize, offset, s.is_done())
    };
    let mut data = std::mem::take(buf);
    data.truncate(n);
    if n > 0 {
        let file = shared.file.clone();
        tokio::task::spawn_blocking(move || file.write_all_at(&data, offset))
            .await
            .map_err(|e| EngineError::Msg(e.to_string()))??;
    }
    Ok(done)
}

async fn run_single(
    cfg: &Config,
    client: Client,
    file: Arc<File>,
    cancel: CancellationToken,
    on_progress: Arc<dyn Fn(Progress) + Send + Sync>,
) -> Result<Outcome, EngineError> {
    let resp = tokio::select! {
        r = apply_headers(client.get(&cfg.url), &cfg.headers).send() => r?,
        _ = cancel.cancelled() => return Ok(Outcome::Stopped),
    };
    if !resp.status().is_success() {
        return Err(EngineError::Http(resp.status().as_u16()));
    }
    let total = resp.content_length().or(cfg.total).filter(|n| *n > 0);
    let mut stream = resp.bytes_stream();
    let mut received: u64 = 0;
    let mut buf: Vec<u8> = Vec::with_capacity(FLUSH_BYTES * 2);
    let mut last = Instant::now();

    macro_rules! write_buf {
        () => {{
            if !buf.is_empty() {
                let data = std::mem::take(&mut buf);
                let len = data.len() as u64;
                let f = file.clone();
                let off = received;
                tokio::task::spawn_blocking(move || f.write_all_at(&data, off))
                    .await
                    .map_err(|e| EngineError::Msg(e.to_string()))??;
                received += len;
            }
        }};
    }
    let report = |received: u64| {
        on_progress(Progress { received, segments: Vec::new() });
    };

    loop {
        let next = tokio::select! {
            n = tokio::time::timeout(STALL_TIMEOUT, stream.next()) => n,
            _ = cancel.cancelled() => {
                write_buf!();
                report(received);
                return Ok(Outcome::Stopped);
            }
        };
        match next {
            Err(_) => return Err(EngineError::Interrupted("Bağlantı zaman aşımına uğradı".into())),
            Ok(None) => break,
            Ok(Some(Err(e))) => return Err(e.into()),
            Ok(Some(Ok(chunk))) => {
                cfg.limiter.acquire(chunk.len()).await;
                buf.extend_from_slice(&chunk);
                if buf.len() >= FLUSH_BYTES {
                    write_buf!();
                }
                if last.elapsed() >= Duration::from_millis(250) {
                    last = Instant::now();
                    report(received + buf.len() as u64);
                }
            }
        }
    }
    write_buf!();
    report(received);
    if let Some(t) = total {
        if received < t {
            return Err(EngineError::Interrupted(format!("Bağlantı erken kapandı ({received}/{t} bayt)")));
        }
    }
    let f = file.clone();
    tokio::task::spawn_blocking(move || f.sync_data()).await.ok();
    Ok(Outcome::Completed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_covers_whole_file() {
        for total in [1u64, 1000, 1_048_576, 10_000_019] {
            let segs = plan_segments(total, 8, 512 * 1024);
            assert_eq!(segs.first().unwrap().start, 0);
            assert_eq!(segs.last().unwrap().end, total - 1);
            for w in segs.windows(2) {
                assert_eq!(w[0].end + 1, w[1].start);
            }
            assert_eq!(segs.iter().map(|s| s.len()).sum::<u64>(), total);
        }
    }

    #[test]
    fn small_files_use_single_segment() {
        assert_eq!(plan_segments(100_000, 8, 512 * 1024).len(), 1);
        assert_eq!(plan_segments(100 * 1024 * 1024, 8, 512 * 1024).len(), 8);
    }
}
