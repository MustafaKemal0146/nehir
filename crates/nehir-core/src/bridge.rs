//! Tarayıcı eklentisinin konuştuğu yerel HTTP köprüsü (127.0.0.1).
//!
//! Güvenlik: yalnızca tarayıcı **eklentileri** istek atabilmeli, sıradan web sayfaları atamamalı.
//! * `Host` başlığı yalnızca loopback adreslerinden biri olmalı (DNS rebinding'e karşı).
//! * `Origin` varsa `chrome-extension://` veya `moz-extension://` olmalı; web kökenleri reddedilir.
//! * `Origin` yoksa (Chrome, host izni olan eklenti GET'lerinde göndermez) özel `X-Nehir-Extension`
//!   başlığı gerekir. Bir web sayfası bu başlığı CORS ön isteği olmadan ekleyemez, ön istek de
//!   web kökeniyle gelip reddedilir.

use crate::manager::Manager;
use crate::model::NewDownload;
use axum::body::Bytes;
use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub const APP_NAME: &str = "Nehir";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

struct Ctx {
    manager: Manager,
    port: u16,
}

pub async fn bind(port: u16) -> std::io::Result<TcpListener> {
    TcpListener::bind(("127.0.0.1", port)).await
}

pub async fn serve(listener: TcpListener, manager: Manager, shutdown: CancellationToken) -> std::io::Result<()> {
    let port = listener.local_addr()?.port();
    let ctx = Arc::new(Ctx { manager, port });
    let app = Router::new()
        .route("/ping", get(ping))
        .route("/downloads", get(downloads))
        .route("/download", post(add_download))
        .route("/action", post(action))
        .route("/formats", post(formats))
        .layer(middleware::from_fn_with_state(ctx.clone(), guard))
        .with_state(ctx);
    axum::serve(listener, app).with_graceful_shutdown(async move { shutdown.cancelled().await }).await
}

pub fn origin_allowed(origin: Option<&str>) -> bool {
    match origin {
        Some(o) if !o.is_empty() && o != "null" => o.starts_with("chrome-extension://") || o.starts_with("moz-extension://"),
        _ => false,
    }
}

fn header_str<'a>(h: &'a HeaderMap, name: &str) -> Option<&'a str> {
    h.get(name).and_then(|v| v.to_str().ok())
}

fn host_allowed(host: Option<&str>, port: u16) -> bool {
    match host.map(str::to_ascii_lowercase) {
        Some(h) => h == format!("127.0.0.1:{port}") || h == format!("localhost:{port}") || h == format!("[::1]:{port}"),
        None => false,
    }
}

fn browser_of(h: &HeaderMap) -> String {
    if let Some(b) = header_str(h, "x-nehir-extension").map(str::to_ascii_lowercase) {
        if ["chrome", "brave", "edge", "firefox", "chromium", "vivaldi", "opera"].contains(&b.as_str()) {
            return b;
        }
    }
    match header_str(h, "origin") {
        Some(o) if o.starts_with("moz-extension://") => "firefox".into(),
        Some(o) if o.starts_with("chrome-extension://") => "chrome".into(),
        _ => "other".into(),
    }
}

async fn guard(State(ctx): State<Arc<Ctx>>, req: Request, next: Next) -> Response {
    let h = req.headers();
    let origin = header_str(h, "origin").map(str::to_string);
    let has_ext_header = header_str(h, "x-nehir-extension").map(|s| !s.is_empty()).unwrap_or(false);
    let from_extension = origin_allowed(origin.as_deref()) || (origin.is_none() && has_ext_header);
    if !from_extension || !host_allowed(header_str(h, "host"), ctx.port) {
        return (StatusCode::FORBIDDEN, Json(json!({"ok": false, "error": "forbidden"}))).into_response();
    }
    ctx.manager.note_extension(&browser_of(h));

    let is_preflight = req.method() == Method::OPTIONS;
    let mut resp = if is_preflight { (StatusCode::NO_CONTENT, ()).into_response() } else { next.run(req).await };
    let hm = resp.headers_mut();
    hm.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    hm.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    if let Some(o) = origin.as_deref().filter(|o| origin_allowed(Some(o))) {
        if let Ok(v) = HeaderValue::from_str(o) {
            hm.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, v);
            hm.insert(header::VARY, HeaderValue::from_static("Origin"));
            hm.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, HeaderValue::from_static("Content-Type, X-Nehir-Extension, X-Nehir-Extension-Id"));
            hm.insert(header::ACCESS_CONTROL_ALLOW_METHODS, HeaderValue::from_static("GET, POST, OPTIONS"));
            // Chrome'un loopback hedefleri için Private/Local Network Access ön istekleri.
            hm.insert("access-control-allow-private-network", HeaderValue::from_static("true"));
            hm.insert("access-control-allow-local-network", HeaderValue::from_static("true"));
        }
    }
    resp
}

async fn ping(State(ctx): State<Arc<Ctx>>) -> Json<Value> {
    let m = &ctx.manager;
    let s = m.settings();
    Json(json!({
        "ok": true,
        "app": APP_NAME,
        "version": VERSION,
        "ytdlp": m.ytdlp_path().is_some(),
        "active": m.list().iter().filter(|d| d.state.is_active()).count(),
        "captureExtensions": s.capture_extensions,
        "language": s.resolved_language(),
    }))
}

async fn downloads(State(ctx): State<Arc<Ctx>>) -> Json<Value> {
    let items: Vec<Value> = ctx
        .manager
        .list()
        .into_iter()
        .take(50)
        .map(|d| {
            json!({
                "id": d.id,
                "name": d.file_name,
                "state": d.state,
                "progress": d.progress(),
                "speed": d.speed,
                "total": d.total_bytes,
                "received": d.received_bytes,
                "stage": d.stage,
            })
        })
        .collect();
    Json(json!({"ok": true, "items": items}))
}

fn bad(msg: &str) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({"ok": false, "error": msg}))).into_response()
}

async fn add_download(State(ctx): State<Arc<Ctx>>, body: Bytes) -> Response {
    let Ok(req) = serde_json::from_slice::<NewDownload>(&body) else { return bad("invalid request") };
    // Eklenti uzak adres dışında hiçbir yola yazamaz: hedef klasörü yalnızca uygulama belirler.
    let req = NewDownload { save_dir: None, ..req };
    match ctx.manager.add(req) {
        Ok(d) => Json(json!({"ok": true, "id": d.id})).into_response(),
        Err(e) => bad(&e),
    }
}

/// Bir videonun kullanılabilir çözünürlüklerini (ve yaklaşık boyutlarını) yt-dlp'den sorar.
async fn formats(State(ctx): State<Arc<Ctx>>, body: Bytes) -> Response {
    let Ok(v) = serde_json::from_slice::<Value>(&body) else { return bad("invalid request") };
    let Some(url) = v.get("url").and_then(Value::as_str).filter(|u| u.starts_with("http://") || u.starts_with("https://")) else {
        return bad("invalid url");
    };
    let Some(ytdlp) = ctx.manager.ytdlp_path() else {
        return Json(json!({"ok": false, "error": "ytdlp_missing"})).into_response();
    };
    let cookies: Vec<crate::model::CookieEntry> = v.get("cookies").and_then(|c| serde_json::from_value(c.clone()).ok()).unwrap_or_default();
    let paths = ctx.manager.paths().clone();
    let cookie_file = (!cookies.is_empty()).then(|| paths.cookies_dir().join(format!("fmt-{}.txt", Uuid::new_v4())));
    if let Some(f) = &cookie_file {
        if let Some(dir) = f.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = crate::video::write_cookie_file(f, &cookies);
    }
    let res = crate::video::list_formats(
        &ytdlp,
        &paths,
        url,
        v.get("userAgent").and_then(Value::as_str),
        v.get("referer").and_then(Value::as_str),
        cookie_file.as_deref(),
    )
    .await;
    if let Some(f) = &cookie_file {
        let _ = std::fs::remove_file(f);
    }
    match res {
        Ok(l) => Json(json!({"ok": true, "title": l.title, "duration": l.duration, "options": l.options})).into_response(),
        Err(e) => Json(json!({"ok": false, "error": e})).into_response(),
    }
}

async fn action(State(ctx): State<Arc<Ctx>>, body: Bytes) -> Response {
    let Ok(v) = serde_json::from_slice::<Value>(&body) else { return bad("invalid request") };
    let (Some(id), Some(action)) = (
        v.get("id").and_then(Value::as_str).and_then(|s| Uuid::parse_str(s).ok()),
        v.get("action").and_then(Value::as_str),
    ) else {
        return bad("id and action required");
    };
    match action {
        "pause" => ctx.manager.pause(id),
        "resume" => ctx.manager.resume(id),
        "cancel" => ctx.manager.cancel(id),
        "remove" => ctx.manager.remove(id, false),
        _ => return bad("unknown action"),
    }
    Json(json!({"ok": true})).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_rules() {
        assert!(origin_allowed(Some("chrome-extension://abcdef")));
        assert!(origin_allowed(Some("moz-extension://uuid")));
        assert!(!origin_allowed(Some("https://evil.example")));
        assert!(!origin_allowed(Some("null")));
        assert!(!origin_allowed(Some("")));
        assert!(!origin_allowed(None));
    }

    #[test]
    fn host_rules() {
        assert!(host_allowed(Some("127.0.0.1:48213"), 48213));
        assert!(host_allowed(Some("LOCALHOST:48213"), 48213));
        assert!(!host_allowed(Some("evil.com:48213"), 48213));
        assert!(!host_allowed(Some("127.0.0.1:1"), 48213));
        assert!(!host_allowed(None, 48213));
    }
}
