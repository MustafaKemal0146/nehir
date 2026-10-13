use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const DEFAULT_PORT: u16 = 48213;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub download_dir: PathBuf,
    /// İndirmeleri kategoriye göre alt klasörlere koy (Videolar, Belgeler…).
    pub categorize: bool,
    /// Tek dosya için paralel bağlantı sayısı (1–32).
    pub max_connections: u32,
    /// Aynı anda çalışan indirme sayısı.
    pub max_concurrent: u32,
    /// Genel hız sınırı, KB/sn. 0 = sınırsız.
    pub speed_limit_kbps: u64,
    pub port: u16,
    /// Eklentinin yakalayacağı uzantılar. Boş liste = hepsi.
    pub capture_extensions: Vec<String>,
    /// "auto" | "tr" | "en"
    pub language: String,
    pub monitor_clipboard: bool,
    pub notifications: bool,
    pub ytdlp_path: String,
    /// "best" | "2160" | "1440" | "1080" | "720" | "480" | "audio"
    pub video_quality: String,
    /// Bitince tepsiye küçül / kapatınca arka planda çalışmaya devam et.
    pub close_to_tray: bool,
    pub autostart: bool,
    /// Yakalama listesi sürümü: eski ayar dosyalarına yeni varsayılanları (m3u8, mpd) bir kez ekler.
    /// Alan eksikse 0 sayılır (struct varsayılanı değil), böylece eski dosyalar taşınır.
    #[serde(default = "zero")]
    pub list_version: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            download_dir: crate::util::default_download_dir(),
            categorize: true,
            max_connections: 8,
            max_concurrent: 3,
            speed_limit_kbps: 0,
            port: DEFAULT_PORT,
            capture_extensions: default_capture_extensions(),
            language: "auto".into(),
            monitor_clipboard: true,
            notifications: true,
            ytdlp_path: String::new(),
            video_quality: "best".into(),
            close_to_tray: true,
            autostart: false,
            list_version: 2,
        }
    }
}

fn zero() -> u32 {
    0
}

pub fn default_capture_extensions() -> Vec<String> {
    "zip rar 7z tar gz xz bz2 zst tgz iso img deb rpm appimage exe msi apk dmg run jar bin \
     pdf doc docx xls xlsx ppt pptx epub odt ods \
     mp4 mkv avi mov webm m4v flv wmv mpg mpeg ts mp3 flac wav m4a ogg opus aac torrent m3u8 mpd"
        .split_whitespace()
        .map(String::from)
        .collect()
}

impl Settings {
    pub fn load(path: &Path) -> Settings {
        std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Settings>(&b).ok())
            .unwrap_or_default()
            .sanitized()
    }

    /// Arayüz dili: ayar "auto" ise bilgisayarın diline göre ("tr" ya da "en").
    pub fn resolved_language(&self) -> &'static str {
        match self.language.as_str() {
            "tr" => "tr",
            "en" => "en",
            _ => crate::util::system_language(),
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        write_atomic(path, &serde_json::to_vec_pretty(self).unwrap_or_default(), 0o600)
    }

    pub fn sanitized(mut self) -> Settings {
        if self.list_version < 2 {
            for e in ["m3u8", "mpd"] {
                self.capture_extensions.push(e.to_string());
            }
            self.list_version = 2;
        }
        if !matches!(self.language.as_str(), "tr" | "en") {
            self.language = "auto".into();
        }
        self.max_connections = self.max_connections.clamp(1, 32);
        self.max_concurrent = self.max_concurrent.clamp(1, 10);
        self.port = self.port.clamp(1024, 65535);
        for e in &mut self.capture_extensions {
            *e = e.trim().trim_start_matches('.').to_ascii_lowercase();
        }
        self.capture_extensions.retain(|e| !e.is_empty());
        self.capture_extensions.sort();
        self.capture_extensions.dedup();
        if self.download_dir.as_os_str().is_empty() {
            self.download_dir = crate::util::default_download_dir();
        }
        self
    }
}

/// Yarı yazılmış dosya bırakmamak için geçici dosyaya yazıp yeniden adlandırır.
pub fn write_atomic(path: &Path, data: &[u8], mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, data)?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode))?;
    std::fs::rename(&tmp, path)
}

#[derive(Debug, Clone)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
}

impl Paths {
    pub fn default_dirs() -> Paths {
        Paths {
            config_dir: dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("nehir"),
            data_dir: dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("nehir"),
        }
    }
    pub fn settings_file(&self) -> PathBuf {
        self.config_dir.join("settings.json")
    }
    pub fn downloads_file(&self) -> PathBuf {
        self.data_dir.join("downloads.json")
    }
    pub fn bin_dir(&self) -> PathBuf {
        self.data_dir.join("bin")
    }
    pub fn cookies_dir(&self) -> PathBuf {
        self.data_dir.join("cookies")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_resolution() {
        let mut s = Settings::default();
        assert_eq!(s.language, "auto");
        s.language = "tr".into();
        assert_eq!(s.resolved_language(), "tr");
        s.language = "en".into();
        assert_eq!(s.resolved_language(), "en");
        s.language = "xx".into();
        assert_eq!(s.sanitized().language, "auto");
    }

    #[test]
    fn old_settings_files_get_stream_extensions_once() {
        let old: Settings = serde_json::from_str(r#"{"capture_extensions":["zip"]}"#).unwrap();
        assert_eq!(old.list_version, 0);
        let migrated = old.sanitized();
        assert!(migrated.capture_extensions.contains(&"m3u8".to_string()) && migrated.capture_extensions.contains(&"mpd".to_string()));
        assert_eq!(migrated.list_version, 2);
        // Kullanıcı sonradan çıkarırsa tekrar eklenmez.
        let mut again = migrated.clone();
        again.capture_extensions.retain(|e| e != "m3u8");
        assert!(!again.sanitized().capture_extensions.contains(&"m3u8".to_string()));
    }
}
