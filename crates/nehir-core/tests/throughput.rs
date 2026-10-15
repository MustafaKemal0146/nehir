//! Motorun CPU/disk tavanını ölçer (loopback, ağ yok). Çalıştırmak için:
//! cargo test -p nehir-core --release --test throughput -- --ignored --nocapture
use axum::{
    http::{header, HeaderMap},
    response::Response,
    routing::get,
    Router,
};
use nehir_core::engine::{self, build_client, Config, Outcome, SpeedLimit};
use std::time::Instant;
use tokio_util::sync::CancellationToken;

const SIZE: u64 = 768 * 1024 * 1024;

async fn serve(headers: HeaderMap) -> Response {
    let (a, b) = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("bytes="))
        .map(|v| {
            let (a, b) = v.split_once('-').unwrap();
            (a.parse::<u64>().unwrap(), b.parse::<u64>().unwrap_or(SIZE - 1).min(SIZE - 1))
        })
        .unwrap_or((0, SIZE - 1));
    let chunk = axum::body::Bytes::from(vec![0xABu8; 256 * 1024]);
    let mut left = b - a + 1;
    let st = futures_util::stream::unfold((), move |_| {
        let chunk = chunk.clone();
        let n = left.min(chunk.len() as u64);
        left -= n;
        async move { (n > 0).then(|| (Ok::<_, std::io::Error>(chunk.slice(..n as usize)), ())) }
    });
    Response::builder()
        .status(206)
        .header(header::CONTENT_RANGE, format!("bytes {a}-{b}/{SIZE}"))
        .header(header::CONTENT_LENGTH, b - a + 1)
        .header(header::ACCEPT_RANGES, "bytes")
        .body(axum::body::Body::from_stream(st))
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn loopback_throughput_by_connection_count() {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, Router::new().route("/f", get(serve))).await.unwrap() });
    let url = format!("http://{addr}/f");
    let client = build_client().unwrap();
    for conns in std::env::var("CONNS").unwrap_or("1,4,8,16,32".into()).split(',').map(|s| s.parse::<u32>().unwrap()) {
        let dir = tempfile::tempdir().unwrap();
        let t = Instant::now();
        let out = engine::run(
            Config {
                url: url.clone(),
                headers: vec![],
                part_path: dir.path().join("b.part"),
                max_connections: conns,
                min_segment_size: 512 * 1024,
                segments: engine::plan_segments(SIZE, conns, 512 * 1024),
                total: Some(SIZE),
                resume: false,
                limiter: SpeedLimit::new(0),
                max_attempts: 12,
                backoff_scale: 1.0,
                fallback_client: None,
            },
            client.clone(),
            CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap();
        assert_eq!(out, Outcome::Completed);
        let secs = t.elapsed().as_secs_f64();
        println!("{conns:>2} bağlantı: {:>7.0} MB/sn  ({:.2} sn, {} MB)", SIZE as f64 / 1e6 / secs, secs, SIZE / 1_000_000);
    }
}
