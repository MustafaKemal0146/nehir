use axum::{
    extract::Path,
    http::{header, HeaderMap},
    response::Response,
    routing::get,
    Router,
};
use nehir_core::manager::{Manager, Tuning};
use nehir_core::model::*;
use nehir_core::settings::Paths;
use std::time::Duration;
use uuid::Uuid;

fn data(n: usize) -> Vec<u8> {
    (0..n).map(|i| ((i * 13 + (i >> 9) * 5) % 253) as u8).collect()
}

const SIZE: usize = 3 * 1024 * 1024;

async fn serve(Path(name): Path<String>, headers: HeaderMap) -> Response {
    if name == "missing" {
        return Response::builder().status(404).body(axum::body::Body::empty()).unwrap();
    }
    let d = data(SIZE);
    let slow = name.starts_with("slow");
    let range = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("bytes="))
        .map(|v| {
            let (a, b) = v.split_once('-').unwrap();
            (a.parse::<usize>().unwrap(), b.parse::<usize>().unwrap_or(SIZE - 1).min(SIZE - 1))
        });
    let (status, slice, cr) = match range {
        Some((a, b)) => (206, d[a..=b].to_vec(), Some(format!("bytes {a}-{b}/{SIZE}"))),
        None => (200, d, None),
    };
    let chunks: Vec<Vec<u8>> = slice.chunks(16 * 1024).map(|c| c.to_vec()).collect();
    let st = futures_util::stream::unfold(chunks.into_iter(), move |mut it| async move {
        let c = it.next()?;
        if slow {
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        Some((Ok::<_, std::io::Error>(axum::body::Bytes::from(c)), it))
    });
    let mut r = Response::builder()
        .status(status)
        .header(header::CONTENT_LENGTH, slice.len())
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_DISPOSITION, "attachment; filename*=UTF-8''g%C3%BCzel%20video.mp4");
    if let Some(cr) = cr {
        r = r.header(header::CONTENT_RANGE, cr);
    }
    r.body(axum::body::Body::from_stream(st)).unwrap()
}

async fn start_server() -> String {
    let app = Router::new().route("/f/:name", get(serve));
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    format!("http://{addr}/f")
}

fn manager(dir: &std::path::Path) -> Manager {
    let paths = Paths { config_dir: dir.join("cfg"), data_dir: dir.join("data") };
    let m = Manager::new(paths);
    let mut s = m.settings();
    s.download_dir = dir.join("dl");
    s.max_connections = 4;
    s.max_concurrent = 2;
    m.update_settings(s);
    m
}

async fn wait_for(m: &Manager, id: Uuid, f: impl Fn(&Download) -> bool) -> Download {
    for _ in 0..200 {
        if let Some(d) = m.get(id) {
            if f(&d) {
                return d;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("zaman aşımı: {:?}", m.get(id).map(|d| (d.state, d.received_bytes, d.error)));
}

#[tokio::test]
async fn downloads_names_and_categorizes() {
    let base = start_server().await;
    let tmp = tempfile::tempdir().unwrap();
    let m = manager(tmp.path());
    let d = m.add(NewDownload { url: format!("{base}/a"), ..Default::default() }).unwrap();
    let d = wait_for(&m, d.id, |d| d.state == State::Completed).await;
    assert_eq!(d.file_name, "güzel video.mp4");
    assert_eq!(d.category, Category::Video);
    assert_eq!(d.dir, tmp.path().join("dl").join("Videolar"));
    assert_eq!(std::fs::read(d.final_path()).unwrap(), data(SIZE));
    assert!(!d.part_path().exists());

    // Aynı ad: üzerine yazma, numaralandır.
    let d2 = m.add(NewDownload { url: format!("{base}/b"), ..Default::default() }).unwrap();
    let d2 = wait_for(&m, d2.id, |d| d.state == State::Completed).await;
    assert_eq!(d2.file_name, "güzel video (1).mp4");
}

#[tokio::test]
async fn pause_resume_and_persist_across_restart() {
    let base = start_server().await;
    let tmp = tempfile::tempdir().unwrap();
    let m = manager(tmp.path());
    let d = m.add(NewDownload { url: format!("{base}/slow1"), ..Default::default() }).unwrap();
    wait_for(&m, d.id, |d| d.state == State::Downloading && d.received_bytes > 200_000).await;
    m.pause(d.id);
    let paused = wait_for(&m, d.id, |d| d.state == State::Paused).await;
    assert!(paused.received_bytes > 0 && (paused.received_bytes as i64) < paused.total_bytes);
    drop(m);

    // "Yeniden başlat": yeni yönetici aynı dizinlerle.
    let m2 = manager(tmp.path());
    let reloaded = m2.get(d.id).expect("kayıt kalıcı olmalı");
    assert_eq!(reloaded.state, State::Paused);
    assert_eq!(reloaded.received_bytes, paused.received_bytes);
    m2.resume(d.id);
    let done = wait_for(&m2, d.id, |d| d.state == State::Completed).await;
    assert_eq!(std::fs::read(done.final_path()).unwrap(), data(SIZE));
}

#[tokio::test]
async fn cancel_removes_partial_file() {
    let base = start_server().await;
    let tmp = tempfile::tempdir().unwrap();
    let m = manager(tmp.path());
    let d = m.add(NewDownload { url: format!("{base}/slow2"), ..Default::default() }).unwrap();
    wait_for(&m, d.id, |d| d.state == State::Downloading && d.received_bytes > 0).await;
    let part = m.get(d.id).unwrap().part_path();
    assert!(part.exists());
    m.cancel(d.id);
    wait_for(&m, d.id, |d| d.state == State::Cancelled).await;
    assert!(!part.exists());
}

#[tokio::test]
async fn queue_respects_concurrency_limit() {
    let base = start_server().await;
    let tmp = tempfile::tempdir().unwrap();
    let m = manager(tmp.path()); // max_concurrent = 2
    let ids: Vec<Uuid> = (0..4)
        .map(|i| m.add(NewDownload { url: format!("{base}/slow{i}"), filename: Some(format!("f{i}.bin")), ..Default::default() }).unwrap().id)
        .collect();
    tokio::time::sleep(Duration::from_millis(400)).await;
    let active = m.list().iter().filter(|d| d.state.is_active()).count();
    let queued = m.list().iter().filter(|d| d.state == State::Queued).count();
    assert_eq!(active, 2);
    assert_eq!(queued, 2);
    for id in ids {
        wait_for(&m, id, |d| d.state == State::Completed).await;
    }
}

#[tokio::test]
async fn rejects_bad_urls_and_dedupes() {
    let tmp = tempfile::tempdir().unwrap();
    let m = manager(tmp.path());
    assert!(m.add(NewDownload { url: "file:///etc/passwd".into(), ..Default::default() }).is_err());
    assert!(m.add(NewDownload { url: "javascript:alert(1)".into(), ..Default::default() }).is_err());
    assert!(m.add(NewDownload { url: "not a url".into(), ..Default::default() }).is_err());
    let a = m.add(NewDownload { url: "http://127.0.0.1:1/x.zip".into(), ..Default::default() }).unwrap();
    let b = m.add(NewDownload { url: "http://127.0.0.1:1/x.zip".into(), ..Default::default() }).unwrap();
    assert_eq!(a.id, b.id);
}

#[tokio::test]
async fn permanent_errors_fail_and_can_be_retried() {
    let base = start_server().await;
    let tmp = tempfile::tempdir().unwrap();
    let m = manager(tmp.path());
    let d = m.add(NewDownload { url: format!("{base}/missing"), ..Default::default() }).unwrap();
    let f = wait_for(&m, d.id, |d| d.state == State::Failed).await;
    assert!(f.error.as_deref().unwrap_or("").contains("404"), "{:?}", f.error);
    assert_eq!(f.retries, 0, "kalıcı hata beklemeye alınmamalı");
    m.resume(d.id);
    wait_for(&m, d.id, |d| d.state == State::Failed).await;
}

#[tokio::test]
async fn unreachable_server_waits_instead_of_failing() {
    let tmp = tempfile::tempdir().unwrap();
    let m = manager(tmp.path());
    m.set_tuning(Tuning { max_attempts: 1, backoff_scale: 0.01, retry_unit_ms: 100, probe_ms: 100, tick_ms: 50 });
    // Kapalı port: bağlantı reddedilir → ağ yokmuş gibi davranır.
    let d = m.add(NewDownload { url: "http://127.0.0.1:1/x.zip".into(), ..Default::default() }).unwrap();
    let w = wait_for(&m, d.id, |d| d.state == State::Waiting).await;
    assert!(w.error.is_some() && w.retry_at.is_some());
}

#[tokio::test]
async fn resumes_automatically_when_server_comes_back() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    let up = Arc::new(AtomicBool::new(false));
    let up2 = up.clone();
    // "up" bayrağı kalkana kadar her istek 503 döner (ağ/sunucu sorunu simülasyonu).
    let app = Router::new().route(
        "/f/:name",
        get(move |p: Path<String>, h: HeaderMap| {
            let up = up2.clone();
            async move {
                if !up.load(Ordering::SeqCst) {
                    return Response::builder().status(503).body(axum::body::Body::empty()).unwrap();
                }
                serve(p, h).await
            }
        }),
    );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });

    let tmp = tempfile::tempdir().unwrap();
    let m = manager(tmp.path());
    m.set_tuning(Tuning { max_attempts: 1, backoff_scale: 0.01, retry_unit_ms: 100, probe_ms: 100, tick_ms: 50 });
    let d = m.add(NewDownload { url: format!("http://{addr}/f/ok"), filename: Some("geri-geldi.bin".into()), ..Default::default() }).unwrap();
    wait_for(&m, d.id, |d| d.state == State::Waiting).await;
    up.store(true, Ordering::SeqCst); // sunucu/ağ geri geldi
    let done = wait_for(&m, d.id, |d| d.state == State::Completed).await;
    assert_eq!(std::fs::read(done.final_path()).unwrap(), data(SIZE));
    assert_eq!(done.error, None);
}

#[tokio::test]
async fn interrupted_download_resumes_by_itself_after_restart() {
    let base = start_server().await;
    let tmp = tempfile::tempdir().unwrap();
    let m = manager(tmp.path());
    let d = m.add(NewDownload { url: format!("{base}/slow1"), ..Default::default() }).unwrap();
    wait_for(&m, d.id, |d| d.state == State::Downloading && d.received_bytes > 200_000).await;
    tokio::time::sleep(Duration::from_millis(1500)).await; // periyodik kayıt yazsın
    drop(m); // bilgisayar kapandı: kullanıcı duraklatmadı

    let m2 = manager(tmp.path());
    let done = wait_for(&m2, d.id, |d| d.state == State::Completed).await;
    assert_eq!(done.error, None);
}

#[tokio::test]
async fn pausing_a_waiting_download_stops_retries() {
    let tmp = tempfile::tempdir().unwrap();
    let m = manager(tmp.path());
    m.set_tuning(Tuning { max_attempts: 1, backoff_scale: 0.01, retry_unit_ms: 100, probe_ms: 100, tick_ms: 50 });
    let d = m.add(NewDownload { url: "http://127.0.0.1:1/x.zip".into(), ..Default::default() }).unwrap();
    wait_for(&m, d.id, |d| d.state == State::Waiting).await;
    m.pause(d.id);
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(m.get(d.id).unwrap().state, State::Paused);
}

#[tokio::test]
async fn video_page_falls_back_to_sniffed_media_urls() {
    let base = start_server().await;
    let tmp = tempfile::tempdir().unwrap();
    let m = manager(tmp.path());
    // Sayfa adresi indirilemez (404 / yt-dlp yok); ikinci yedek geçerli bir medya dosyası.
    let d = m
        .add(NewDownload {
            url: format!("{base}/missing"),
            kind: Kind::Video,
            fallback_urls: vec![format!("{base}/missing"), format!("{base}/clip")],
            cookie_header: Some("session=secret".into()),
            ..Default::default()
        })
        .unwrap();
    let done = wait_for(&m, d.id, |d| d.state == State::Completed).await;
    assert_eq!(std::fs::read(done.final_path()).unwrap(), data(SIZE));
    assert!(done.url.ends_with("/clip"));
    assert_eq!(done.kind, Kind::File);
}

#[tokio::test]
async fn detected_media_is_named_after_page_title() {
    let base = start_server().await;
    let tmp = tempfile::tempdir().unwrap();
    let m = manager(tmp.path());
    // Sunucu Content-Disposition ile "güzel video.mp4" önerir; sayfa başlığı ipucu önceliklidir.
    let d = m
        .add(NewDownload { url: format!("{base}/850x480"), title_hint: Some("Güzel: Bir/Video".into()), ..Default::default() })
        .unwrap();
    let done = wait_for(&m, d.id, |d| d.state == State::Completed).await;
    assert_eq!(done.file_name, "Güzel: Bir-Video.mp4");
}

#[tokio::test]
async fn partial_files_live_in_hidden_work_dir() {
    let base = start_server().await;
    let tmp = tempfile::tempdir().unwrap();
    let m = manager(tmp.path());
    let d = m.add(NewDownload { url: format!("{base}/slow-hidden"), filename: Some("kutu.zip".into()), ..Default::default() }).unwrap();
    let running = wait_for(&m, d.id, |d| d.state == State::Downloading && d.received_bytes > 100_000).await;
    // İndirme klasöründe (kategori klasörü dahil) görünür yarım dosya olmamalı…
    let visible: Vec<_> = std::fs::read_dir(&running.dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| !n.starts_with('.')).collect();
    assert!(visible.is_empty(), "görünür dosya var: {visible:?}");
    // …yarım dosya gizli çalışma klasöründe durmalı.
    assert!(running.work_dir().join("kutu.zip.nehirpart").exists());
    let done = wait_for(&m, d.id, |d| d.state == State::Completed).await;
    assert!(done.final_path().exists());
    assert!(!done.work_dir().exists(), "çalışma klasörü boş kalınca silinmeli");
}

#[tokio::test]
async fn audio_requests_go_to_music_folder_and_keep_quality() {
    let tmp = tempfile::tempdir().unwrap();
    let m = manager(tmp.path());
    let d = m
        .add(NewDownload { url: "https://example.com/watch?v=1".into(), kind: Kind::Video, quality: Some("audio".into()), ..Default::default() })
        .unwrap();
    assert_eq!(d.category, Category::Music);
    assert_eq!(d.dir, tmp.path().join("dl").join("Müzik"));
    assert_eq!(d.quality.as_deref(), Some("audio"));
    // Geçersiz kalite değeri yok sayılır (komut satırına sızmasın).
    let d2 = m
        .add(NewDownload { url: "https://example.com/watch?v=2".into(), kind: Kind::Video, quality: Some("--exec rm".into()), ..Default::default() })
        .unwrap();
    assert_eq!(d2.quality, None);
}
