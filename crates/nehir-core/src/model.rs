use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

/// Bir dosyanın parçası: `[start, end]` (kapalı aralık) ve şimdiye kadar diske yazılan bayt sayısı.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Segment {
    pub start: u64,
    pub end: u64,
    pub received: u64,
}

impl Segment {
    pub fn len(&self) -> u64 {
        self.end + 1 - self.start
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn remaining(&self) -> u64 {
        self.len().saturating_sub(self.received)
    }
    pub fn is_done(&self) -> bool {
        self.received >= self.len()
    }
    /// Bir sonraki okunacak mutlak bayt konumu.
    pub fn cursor(&self) -> u64 {
        self.start + self.received
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum State {
    #[default]
    Queued,
    Probing,
    Downloading,
    /// Ağ/sunucu hatası sonrası otomatik yeniden deneme bekleniyor.
    Waiting,
    Paused,
    Processing,
    Completed,
    Failed,
    Cancelled,
}

impl State {
    pub fn is_active(self) -> bool {
        matches!(self, State::Probing | State::Downloading | State::Processing)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    File,
    Video,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Video,
    Music,
    Document,
    Archive,
    Program,
    Image,
    Other,
}

impl Category {
    pub fn from_file_name(name: &str) -> Category {
        let ext = name
            .rsplit_once('.')
            .map(|(_, e)| e.to_ascii_lowercase())
            .unwrap_or_default();
        match ext.as_str() {
            "mp4" | "mkv" | "webm" | "avi" | "mov" | "flv" | "m4v" | "wmv" | "ts" | "mpg" | "mpeg" => Category::Video,
            "mp3" | "m4a" | "flac" | "wav" | "ogg" | "opus" | "aac" | "wma" => Category::Music,
            "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "odt" | "ods" | "txt" | "epub" | "csv" => Category::Document,
            "zip" | "rar" | "7z" | "tar" | "gz" | "xz" | "bz2" | "zst" | "tgz" | "iso" => Category::Archive,
            "deb" | "rpm" | "appimage" | "exe" | "msi" | "apk" | "sh" | "run" | "flatpakref" | "snap" => Category::Program,
            "jpg" | "jpeg" | "png" | "gif" | "webp" | "svg" | "bmp" | "tiff" | "heic" => Category::Image,
            _ => Category::Other,
        }
    }

    pub fn folder_name(self) -> &'static str {
        match self {
            Category::Video => "Videolar",
            Category::Music => "Müzik",
            Category::Document => "Belgeler",
            Category::Archive => "Sıkıştırılmış",
            Category::Program => "Programlar",
            Category::Image => "Resimler",
            Category::Other => "Diğer",
        }
    }
}

/// Eklentiden / kullanıcıdan gelen yeni indirme isteği.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NewDownload {
    pub url: String,
    #[serde(default)]
    pub filename: Option<String>,
    #[serde(default)]
    pub referer: Option<String>,
    #[serde(default, rename = "userAgent")]
    pub user_agent: Option<String>,
    #[serde(default, rename = "cookieHeader")]
    pub cookie_header: Option<String>,
    /// Netscape cookie dosyası üretmek için (yt-dlp).
    #[serde(default)]
    pub cookies: Vec<CookieEntry>,
    #[serde(default)]
    pub kind: Kind,
    #[serde(default)]
    pub headers: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub segments: Option<u32>,
    /// Hedef klasör (yoksa ayarlardaki / kategoriye göre).
    #[serde(default, rename = "saveDir")]
    pub save_dir: Option<PathBuf>,
    /// Video sayfası yt-dlp ile indirilemezse sırayla denenecek, sayfada algılanmış medya adresleri.
    #[serde(default, rename = "fallbackUrls")]
    pub fallback_urls: Vec<String>,
    /// Sayfa başlığı: akış/medya adreslerinde (m3u8, 850x480.mp4 …) dosya adı olarak kullanılır.
    #[serde(default, rename = "titleHint")]
    pub title_hint: Option<String>,
    /// Video kalitesini bu indirme için değiştirir: "audio" (MP3), "best", "1080" …
    #[serde(default)]
    pub quality: Option<String>,
    /// Beklenen dosya özeti ("sha256:…" ya da yalnızca onaltılık metin). Varsa indirme bitince doğrulanır.
    #[serde(default)]
    pub checksum: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CookieEntry {
    pub name: String,
    pub value: String,
    #[serde(default)]
    pub domain: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub secure: bool,
    #[serde(default, rename = "expirationDate")]
    pub expiration_date: f64,
}

/// Diskte saklanan ve arayüze giden indirme kaydı.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Download {
    pub id: Uuid,
    pub url: String,
    pub file_name: String,
    pub dir: PathBuf,
    pub kind: Kind,
    pub category: Category,
    pub state: State,
    pub total_bytes: i64,
    pub received_bytes: u64,
    pub speed: f64,
    pub segments: Vec<Segment>,
    pub segment_count: u32,
    pub supports_ranges: bool,
    pub error: Option<String>,
    /// Video indirmelerinde aşama metni ("Ses indiriliyor…" gibi).
    pub stage: Option<String>,
    pub created_at: u64,
    pub finished_at: Option<u64>,
    // Aşağıdakiler yeniden başlatmada gerekli.
    pub referer: Option<String>,
    pub user_agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cookie_header: Option<String>,
    #[serde(default)]
    pub extra_headers: std::collections::BTreeMap<String, String>,
    /// Hedef klasör kullanıcı/eklenti tarafından açıkça verildi: kategoriye göre taşıma.
    #[serde(default)]
    pub explicit_dir: bool,
    #[serde(default)]
    pub fallback_urls: Vec<String>,
    #[serde(default)]
    pub title_hint: Option<String>,
    #[serde(default)]
    pub quality: Option<String>,
    /// Art arda ilerleme olmadan yapılan otomatik yeniden deneme sayısı.
    #[serde(default)]
    pub retries: u32,
    /// Bir sonraki otomatik denemenin zamanı (unix saniye).
    #[serde(default)]
    pub retry_at: Option<u64>,
    /// Son denemede ulaşılan bayt: ilerleme olduysa `retries` sıfırlanır.
    #[serde(default)]
    pub retry_mark: u64,
    /// Kullanılan protokol ("HTTP/1.1", "HTTP/2").
    #[serde(default)]
    pub protocol: Option<String>,
    /// Beklenen özet ("sha256:…"), hesaplanan özet ve karşılaştırma sonucu.
    #[serde(default)]
    pub checksum_expected: Option<String>,
    #[serde(default)]
    pub checksum_actual: Option<String>,
    #[serde(default)]
    pub checksum_ok: Option<bool>,
}

impl Download {
    pub fn audio_only(&self) -> bool {
        self.quality.as_deref() == Some("audio")
    }
}

/// Yarım dosyaların ve ara dosyaların tutulduğu gizli klasör (indirme klasöründe görünmesin diye).
pub const WORK_DIR_NAME: &str = ".nehir-tmp";

impl Download {
    pub fn progress(&self) -> f64 {
        if self.total_bytes > 0 {
            (self.received_bytes as f64 / self.total_bytes as f64).clamp(0.0, 1.0)
        } else if self.state == State::Completed {
            1.0
        } else {
            0.0
        }
    }

    pub fn final_path(&self) -> PathBuf {
        self.dir.join(&self.file_name)
    }

    pub fn work_dir(&self) -> PathBuf {
        self.dir.join(WORK_DIR_NAME)
    }

    pub fn part_path(&self) -> PathBuf {
        self.work_dir().join(format!("{}.nehirpart", self.file_name))
    }
}
