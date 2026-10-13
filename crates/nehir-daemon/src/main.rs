//! Arayüzsüz Nehir: indirme yöneticisi + tarayıcı köprüsü. Sunucu/terminal kullanımı ve testler için.
//! Kullanım: nehir-daemon [--port 48213] [--dir ~/Downloads] [--data-dir <yol>]

use nehir_core::{bridge, manager::{Event, Manager}, settings::Paths};
use tokio_util::sync::CancellationToken;

fn arg(name: &str) -> Option<String> {
    let a: Vec<String> = std::env::args().collect();
    a.iter().position(|x| x == name).and_then(|i| a.get(i + 1).cloned())
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into())).init();
    let mut paths = Paths::default_dirs();
    if let Some(d) = arg("--data-dir") {
        let d = std::path::PathBuf::from(d);
        paths = Paths { config_dir: d.join("config"), data_dir: d.join("data") };
    }
    let manager = Manager::new(paths);
    let mut s = manager.settings();
    if let Some(p) = arg("--port").and_then(|p| p.parse().ok()) {
        s.port = p;
    }
    if let Some(d) = arg("--dir") {
        s.download_dir = d.into();
    }
    let s = manager.update_settings(s);

    let listener = match bridge::bind(s.port).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("127.0.0.1:{} dinlenemiyor: {e} (Nehir zaten çalışıyor olabilir)", s.port);
            std::process::exit(1);
        }
    };
    tracing::info!("Nehir köprüsü 127.0.0.1:{} üzerinde dinliyor, indirme klasörü: {}", s.port, s.download_dir.display());
    let stop = CancellationToken::new();
    tokio::spawn(bridge::serve(listener, manager.clone(), stop.clone()));

    let mut rx = manager.subscribe();
    tokio::spawn(async move {
        while let Ok(ev) = rx.recv().await {
            match ev {
                Event::Completed(d) => tracing::info!("tamamlandı: {}", d.final_path().display()),
                Event::Failed(d) => tracing::warn!("başarısız: {} — {}", d.file_name, d.error.unwrap_or_default()),
                _ => {}
            }
        }
    });

    let _ = tokio::signal::ctrl_c().await;
    manager.pause_all();
    stop.cancel();
}
