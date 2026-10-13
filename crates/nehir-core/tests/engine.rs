use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use nehir_core::engine::{self, build_client, Config, Outcome, SpeedLimit};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct Srv {
    data: Arc<Vec<u8>>,
    ranges: bool,
    /// Range desteğini bildirir ama her zaman 200 döner.
    lie: bool,
    throttle_first: Arc<AtomicUsize>,
    delay_ms: u64,
    hits: Arc<AtomicUsize>,
}

fn pattern(n: usize) -> Vec<u8> {
    (0..n).map(|i| ((i * 31 + (i >> 8) * 7) % 251) as u8).collect()
}

async fn serve(State(s): State<Srv>, headers: HeaderMap) -> Response {
    s.hits.fetch_add(1, Ordering::SeqCst);
    if s.throttle_first.load(Ordering::SeqCst) > 0 {
        s.throttle_first.fetch_sub(1, Ordering::SeqCst);
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let total = s.data.len();
    let range = headers.get(header::RANGE).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("bytes=")).map(|v| {
        let (a, b) = v.split_once('-').unwrap();
        (a.parse::<usize>().unwrap(), b.parse::<usize>().unwrap_or(total - 1).min(total - 1))
    });
    let body_stream = |slice: Vec<u8>, delay: u64| {
        let chunks: Vec<Vec<u8>> = slice.chunks(32 * 1024).map(|c| c.to_vec()).collect();
        let st = futures_util::stream::unfold(chunks.into_iter(), move |mut it| async move {
            let c = it.next()?;
            if delay > 0 {
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
            Some((Ok::<_, std::io::Error>(axum::body::Bytes::from(c)), it))
        });
        axum::body::Body::from_stream(st)
    };
    match range {
        Some((a, b)) if s.ranges && !s.lie => {
            let slice = s.data[a..=b].to_vec();
            Response::builder()
                .status(206)
                .header(header::CONTENT_RANGE, format!("bytes {a}-{b}/{total}"))
                .header(header::CONTENT_LENGTH, slice.len())
                .header(header::ACCEPT_RANGES, "bytes")
                .body(body_stream(slice, s.delay_ms))
                .unwrap()
        }
        _ => {
            let mut r = Response::builder().status(200).header(header::CONTENT_LENGTH, total);
            if s.ranges || s.lie {
                r = r.header(header::ACCEPT_RANGES, "bytes");
            }
            r.body(body_stream(s.data.to_vec(), s.delay_ms)).unwrap()
        }
    }
}

async fn start(srv: Srv) -> String {
    let app = Router::new().route("/file", get(serve)).with_state(srv);
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    format!("http://{addr}/file")
}

fn srv(size: usize) -> Srv {
    Srv {
        data: Arc::new(pattern(size)),
        ranges: true,
        lie: false,
        throttle_first: Arc::new(AtomicUsize::new(0)),
        delay_ms: 0,
        hits: Arc::new(AtomicUsize::new(0)),
    }
}

async fn download(url: &str, dir: &std::path::Path, conns: u32) -> (Outcome, std::path::PathBuf) {
    let client = build_client().unwrap();
    let info = engine::probe(&client, url, &[]).await.unwrap();
    let segments = if info.ranges {
        engine::plan_segments(info.total.unwrap(), conns, 64 * 1024)
    } else {
        vec![]
    };
    let part = dir.join("out.part");
    let out = engine::run(
        Config {
            url: url.into(),
            headers: vec![],
            part_path: part.clone(),
            max_connections: conns,
            min_segment_size: 64 * 1024,
            segments,
            total: info.total,
            resume: false,
            limiter: SpeedLimit::new(0),
            max_attempts: 12,
            backoff_scale: 1.0,
        },
        client,
        CancellationToken::new(),
        |_| {},
    )
    .await
    .unwrap();
    (out, part)
}

#[tokio::test]
async fn multi_segment_matches_source() {
    let s = srv(5 * 1024 * 1024 + 123);
    let url = start(s.clone()).await;
    let dir = tempfile::tempdir().unwrap();
    let (out, part) = download(&url, dir.path(), 8).await;
    assert_eq!(out, Outcome::Completed);
    assert_eq!(std::fs::read(part).unwrap(), *s.data);
}

#[tokio::test]
async fn no_range_support_falls_back_to_single_stream() {
    let mut s = srv(1_500_000);
    s.ranges = false;
    let url = start(s.clone()).await;
    let dir = tempfile::tempdir().unwrap();
    let (out, part) = download(&url, dir.path(), 8).await;
    assert_eq!(out, Outcome::Completed);
    assert_eq!(std::fs::read(part).unwrap(), *s.data);
}

#[tokio::test]
async fn server_that_lies_about_ranges_still_completes() {
    let mut s = srv(2_000_000);
    s.lie = true;
    let url = start(s.clone()).await;
    let dir = tempfile::tempdir().unwrap();
    let client = build_client().unwrap();
    // Probe 200 döner ve Accept-Ranges bildirir → motor çok parçalı dener, 200 görünce tek akışa düşer.
    let info = engine::probe(&client, &url, &[]).await.unwrap();
    assert!(info.ranges);
    let part = dir.path().join("o.part");
    let out = engine::run(
        Config {
            url: url.clone(),
            headers: vec![],
            part_path: part.clone(),
            max_connections: 4,
            min_segment_size: 64 * 1024,
            segments: engine::plan_segments(info.total.unwrap(), 4, 64 * 1024),
            total: info.total,
            resume: false,
            limiter: SpeedLimit::new(0),
            max_attempts: 12,
            backoff_scale: 1.0,
        },
        client,
        CancellationToken::new(),
        |_| {},
    )
    .await
    .unwrap();
    assert_eq!(out, Outcome::Completed);
    assert_eq!(std::fs::read(part).unwrap(), *s.data);
}

#[tokio::test]
async fn recovers_from_503_throttling() {
    let s = srv(3 * 1024 * 1024);
    let url = start(s.clone()).await;
    // Probe'dan sonra ilk 5 istek 503 dönsün.
    let client = build_client().unwrap();
    let info = engine::probe(&client, &url, &[]).await.unwrap();
    s.throttle_first.store(5, Ordering::SeqCst);
    let dir = tempfile::tempdir().unwrap();
    let part = dir.path().join("t.part");
    let out = engine::run(
        Config {
            url,
            headers: vec![],
            part_path: part.clone(),
            max_connections: 6,
            min_segment_size: 64 * 1024,
            segments: engine::plan_segments(info.total.unwrap(), 6, 64 * 1024),
            total: info.total,
            resume: false,
            limiter: SpeedLimit::new(0),
            max_attempts: 12,
            backoff_scale: 1.0,
        },
        client,
        CancellationToken::new(),
        |_| {},
    )
    .await
    .unwrap();
    assert_eq!(out, Outcome::Completed);
    assert_eq!(std::fs::read(part).unwrap(), *s.data);
}

#[tokio::test]
async fn pause_then_resume_produces_identical_file() {
    let mut s = srv(8 * 1024 * 1024);
    s.delay_ms = 25; // yavaşlat ki ortada durdurabilelim
    let url = start(s.clone()).await;
    let client = build_client().unwrap();
    let info = engine::probe(&client, &url, &[]).await.unwrap();
    let total = info.total.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let part = dir.path().join("r.part");
    let saved: Arc<Mutex<Option<engine::Progress>>> = Arc::new(Mutex::new(None));

    let cancel = CancellationToken::new();
    let c2 = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(400)).await;
        c2.cancel();
    });
    let sv = saved.clone();
    let out = engine::run(
        Config {
            url: url.clone(),
            headers: vec![],
            part_path: part.clone(),
            max_connections: 4,
            min_segment_size: 64 * 1024,
            segments: engine::plan_segments(total, 4, 64 * 1024),
            total: Some(total),
            resume: false,
            limiter: SpeedLimit::new(0),
            max_attempts: 12,
            backoff_scale: 1.0,
        },
        client.clone(),
        cancel,
        move |p| *sv.lock().unwrap() = Some(p),
    )
    .await
    .unwrap();
    assert_eq!(out, Outcome::Stopped);
    let p = saved.lock().unwrap().clone().unwrap();
    assert!(p.received > 0 && p.received < total, "ara durumda olmalı: {}", p.received);

    // Devam: kaydedilen parçalarla.
    s.delay_ms = 0;
    let out = engine::run(
        Config {
            url,
            headers: vec![],
            part_path: part.clone(),
            max_connections: 4,
            min_segment_size: 64 * 1024,
            segments: p.segments,
            total: Some(total),
            resume: true,
            limiter: SpeedLimit::new(0),
            max_attempts: 12,
            backoff_scale: 1.0,
        },
        client,
        CancellationToken::new(),
        |_| {},
    )
    .await
    .unwrap();
    assert_eq!(out, Outcome::Completed);
    assert_eq!(std::fs::read(part).unwrap(), *s.data);
}

#[tokio::test]
async fn speed_limit_is_respected() {
    let s = srv(1_000_000);
    let url = start(s.clone()).await;
    let client = build_client().unwrap();
    let info = engine::probe(&client, &url, &[]).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let t = std::time::Instant::now();
    let out = engine::run(
        Config {
            url,
            headers: vec![],
            part_path: dir.path().join("l.part"),
            max_connections: 4,
            min_segment_size: 64 * 1024,
            segments: engine::plan_segments(info.total.unwrap(), 4, 64 * 1024),
            total: info.total,
            resume: false,
            limiter: SpeedLimit::new(500_000),
            max_attempts: 12,
            backoff_scale: 1.0, // 500 KB/s → ~2 sn
        },
        client,
        CancellationToken::new(),
        |_| {},
    )
    .await
    .unwrap();
    assert_eq!(out, Outcome::Completed);
    assert!(t.elapsed() > Duration::from_millis(1400), "çok hızlı bitti: {:?}", t.elapsed());
}
