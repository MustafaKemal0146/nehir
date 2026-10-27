//! Gerçek ağ ve gerçek araçlarla Windows uçtan uca denemesi (yt-dlp + deno + ffmpeg kurulumu, video indirme,
//! iptalde süreç ağacının ölmesi). Yüzlerce MB indirdiği için varsayılan olarak çalışmaz:
//!
//!     cargo test -p nehir-core --test windows_tools -- --ignored --nocapture --test-threads=1
#![cfg(windows)]

use nehir_core::settings::Paths;
use nehir_core::video::{self, VideoOpts};
use std::process::Command;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const TEST_VIDEO: &str = "https://www.youtube.com/watch?v=jNQXAC9IVRw"; // "Me at the zoo": YouTube'un ilk videosu (19 sn)

fn paths(tmp: &std::path::Path) -> Paths {
    Paths { config_dir: tmp.join("c"), data_dir: tmp.join("d") }
}

fn opts(paths: &Paths, tmp: &std::path::Path, name: &str) -> VideoOpts {
    VideoOpts {
        url: TEST_VIDEO.into(),
        out_dir: tmp.join(name).join("out"),
        quality: "best".into(),
        user_agent: None,
        referer: None,
        cookie_file: None,
        ytdlp: video::locate_ytdlp("", paths).expect("yt-dlp kurulu olmalı"),
        connections: 4,
        work_dir: tmp.join(name).join("work"),
        name_stem: None,
    }
}

fn running(image: &str) -> bool {
    let out = Command::new("tasklist").args(["/FI", &format!("IMAGENAME eq {image}"), "/NH"]).output().unwrap();
    String::from_utf8_lossy(&out.stdout).to_ascii_lowercase().contains(&image.to_ascii_lowercase())
}

#[tokio::test]
#[ignore = "ağ kullanır, ~200 MB indirir"]
async fn install_tools_download_video_and_cancel_kills_the_tree() {
    let tmp = tempfile::tempdir().unwrap();
    let p = paths(tmp.path());
    // Makinede PATH'te ffmpeg varsa kurucu onu atlar (doğru davranış); kurulum yolunu sınamak için PATH'ten çıkar.
    let path = std::env::var_os("PATH").unwrap_or_default();
    let clean: Vec<_> = std::env::split_paths(&path).filter(|d| !d.join("ffmpeg.exe").exists()).collect();
    std::env::set_var("PATH", std::env::join_paths(clean).unwrap());
    let client = nehir_core::engine::build_client().unwrap();

    // 1) Kurulum: yt-dlp.exe + deno.exe + ffmpeg.exe/ffprobe.exe
    video::install_ytdlp(&client, &p, |_| {}).await.expect("install_ytdlp");
    let ytdlp = video::locate_ytdlp("", &p).expect("yt-dlp bulunamadı");
    assert!(video::locate_deno(&p).is_some(), "deno bulunamadı");
    assert!(video::locate_ffmpeg(&p).is_some(), "ffmpeg bulunamadı");
    assert!(p.bin_dir().join("ffprobe.exe").exists(), "ffprobe bulunamadı");
    let v = Command::new(&ytdlp).arg("--version").output().unwrap();
    println!("yt-dlp {}", String::from_utf8_lossy(&v.stdout).trim());
    assert!(v.status.success());

    // 2) Çözünürlük listesi
    let list = video::list_formats(&ytdlp, &p, TEST_VIDEO, None, None, None).await.expect("list_formats");
    println!("başlık: {:?}, seçenekler: {:?}", list.title, list.options.iter().map(|o| o.height).collect::<Vec<_>>());
    assert!(!list.options.is_empty());

    // 3) İndir (ses+görüntü ffmpeg ile birleşir)
    let o = opts(&p, tmp.path(), "indir");
    let out_dir = o.out_dir.clone();
    let res = video::run(o, &p, CancellationToken::new(), |_| {}, |s| println!("aşama: {s}")).await.expect("video::run");
    let file = res.expect("tamamlanmalı");
    let len = std::fs::metadata(&file).unwrap().len();
    println!("indirilen: {} ({} bayt)", file.display(), len);
    assert!(file.starts_with(&out_dir) && len > 10_000);

    // 4) İptal: ağaç (yt-dlp → deno/ffmpeg) birlikte ölmeli
    let o = opts(&p, tmp.path(), "iptal");
    let cancel = CancellationToken::new();
    let c2 = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(2500)).await;
        c2.cancel();
    });
    let res = video::run(o, &p, cancel, |_| {}, |_| {}).await.expect("iptal hata olmamalı");
    assert!(res.is_none(), "iptal edilen indirme dosya döndürmemeli");
    tokio::time::sleep(Duration::from_millis(1500)).await;
    for image in ["yt-dlp.exe", "deno.exe", "ffmpeg.exe"] {
        assert!(!running(image), "{image} iptalden sonra hâlâ çalışıyor");
    }
}
