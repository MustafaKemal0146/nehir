use nehir_core::bridge;
use nehir_core::manager::Manager;
use nehir_core::settings::Paths;
use tokio_util::sync::CancellationToken;

async fn start() -> (String, u16, tempfile::TempDir, CancellationToken) {
    let tmp = tempfile::tempdir().unwrap();
    let m = Manager::new(Paths { config_dir: tmp.path().join("c"), data_dir: tmp.path().join("d") });
    let l = bridge::bind(0).await.unwrap();
    let port = l.local_addr().unwrap().port();
    let stop = CancellationToken::new();
    tokio::spawn(bridge::serve(l, m, stop.clone()));
    (format!("http://127.0.0.1:{port}"), port, tmp, stop)
}

#[tokio::test]
async fn rejects_web_pages_and_accepts_extensions() {
    let (base, _port, _tmp, stop) = start().await;
    let c = reqwest::Client::new();

    // Başlıksız istek (ör. rastgele yerel süreç / basit form): reddedilir.
    assert_eq!(c.get(format!("{base}/ping")).send().await.unwrap().status(), 403);
    // Web kökeni: reddedilir, CORS başlığı yok.
    let r = c.get(format!("{base}/ping")).header("Origin", "https://evil.example").header("X-Nehir-Extension", "chrome").send().await.unwrap();
    assert_eq!(r.status(), 403);
    assert!(r.headers().get("access-control-allow-origin").is_none());
    let r = c.post(format!("{base}/download")).header("Origin", "null").body("{}").send().await.unwrap();
    assert_eq!(r.status(), 403);
    // DNS rebinding: yanlış Host.
    let r = c.get(format!("{base}/ping")).header("Host", "evil.com").header("X-Nehir-Extension", "chrome").send().await.unwrap();
    assert_eq!(r.status(), 403);

    // Eklenti: Origin'siz + özel başlık.
    let r = c.get(format!("{base}/ping")).header("X-Nehir-Extension", "chrome").send().await.unwrap();
    assert_eq!(r.status(), 200);
    let j: serde_json::Value = r.json().await.unwrap();
    assert_eq!(j["ok"], true);
    assert_eq!(j["app"], "Nehir");
    assert!(j["captureExtensions"].as_array().unwrap().len() > 10);

    // Eklenti: chrome-extension kökeni + CORS yanıtı.
    let r = c.get(format!("{base}/ping")).header("Origin", "chrome-extension://abcdefgh").send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["access-control-allow-origin"], "chrome-extension://abcdefgh");
    assert_eq!(r.headers()["access-control-allow-private-network"], "true");

    // Ön istek.
    let r = c.request(reqwest::Method::OPTIONS, format!("{base}/download")).header("Origin", "moz-extension://uuid").send().await.unwrap();
    assert_eq!(r.status(), 204);
    stop.cancel();
}

#[tokio::test]
async fn download_endpoint_validates_and_ignores_save_dir() {
    let (base, _port, tmp, stop) = start().await;
    let c = reqwest::Client::new();
    let post = |body: &str| c.post(format!("{base}/download")).header("X-Nehir-Extension", "chrome").header("Content-Type", "application/json").body(body.to_string());

    assert_eq!(post("{\"url\":\"file:///etc/passwd\"}").send().await.unwrap().status(), 400);
    assert_eq!(post("not json").send().await.unwrap().status(), 400);

    // Eklenti hedef klasörü seçemez (saveDir yok sayılır).
    let evil = tmp.path().join("evil");
    let body = format!("{{\"url\":\"http://127.0.0.1:1/a.zip\",\"saveDir\":\"{}\"}}", evil.display());
    let r = post(&body).send().await.unwrap();
    assert_eq!(r.status(), 200);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(!evil.exists());

    let list: serde_json::Value = c.get(format!("{base}/downloads")).header("X-Nehir-Extension", "chrome").send().await.unwrap().json().await.unwrap();
    assert_eq!(list["items"].as_array().unwrap().len(), 1);

    let r = c.post(format!("{base}/action")).header("X-Nehir-Extension", "chrome").body("{\"id\":\"nope\",\"action\":\"pause\"}").send().await.unwrap();
    assert_eq!(r.status(), 400);
    stop.cancel();
}

#[tokio::test]
async fn formats_endpoint_lists_resolutions_via_ytdlp() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    // Sahte yt-dlp: sabit bir -J çıktısı verir (gerçek ağ gerekmez).
    let fake = tmp.path().join("fake-yt-dlp");
    std::fs::write(
        &fake,
        r#"#!/bin/sh
echo '{"title":"Deneme","duration":12.5,"formats":[{"vcodec":"none","acodec":"mp4a","ext":"m4a","filesize":1000},{"vcodec":"avc1","acodec":"none","height":1080,"ext":"mp4","filesize":9000},{"vcodec":"avc1","acodec":"none","height":480,"ext":"mp4","filesize":3000}]}'
"#,
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

    let m = Manager::new(Paths { config_dir: tmp.path().join("c"), data_dir: tmp.path().join("d") });
    let mut s = m.settings();
    s.ytdlp_path = fake.to_string_lossy().into_owned();
    m.update_settings(s);
    let l = bridge::bind(0).await.unwrap();
    let port = l.local_addr().unwrap().port();
    let stop = CancellationToken::new();
    tokio::spawn(bridge::serve(l, m, stop.clone()));
    let base = format!("http://127.0.0.1:{port}");
    let c = reqwest::Client::new();

    let r: serde_json::Value = c
        .post(format!("{base}/formats"))
        .header("X-Nehir-Extension", "chrome")
        .body(r#"{"url":"https://example.com/v"}"#)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["ok"], true);
    assert_eq!(r["title"], "Deneme");
    assert_eq!(r["options"][0]["height"], 1080);
    assert_eq!(r["options"][0]["size"], 10000);
    assert_eq!(r["options"][1]["height"], 480);

    // Geçersiz adres: yt-dlp'ye hiç gitmez.
    let r = c.post(format!("{base}/formats")).header("X-Nehir-Extension", "chrome").body(r#"{"url":"file:///etc/passwd"}"#).send().await.unwrap();
    assert_eq!(r.status(), 400);
    stop.cancel();
}
