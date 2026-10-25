use percent_encoding::percent_decode_str;
use std::path::{Path, PathBuf};

/// `Content-Disposition` başlığından dosya adını çıkarır (RFC 5987 `filename*` öncelikli).
pub fn file_name_from_content_disposition(header: &str) -> Option<String> {
    let lower = header.to_ascii_lowercase();
    if let Some(pos) = lower.find("filename*") {
        let rest = &header[pos + "filename*".len()..];
        let rest = rest.trim_start().strip_prefix('=')?.trim_start();
        let value = rest.split(';').next().unwrap_or("").trim().trim_matches('"');
        // charset'lang'değer
        let mut parts = value.splitn(3, '\'');
        let _charset = parts.next();
        let _lang = parts.next();
        if let Some(enc) = parts.next() {
            let decoded = percent_decode_str(enc).decode_utf8_lossy().to_string();
            if !decoded.is_empty() {
                return Some(sanitize_file_name(&decoded));
            }
        }
    }
    let mut search = 0;
    while let Some(rel) = lower[search..].find("filename") {
        let pos = search + rel;
        search = pos + "filename".len();
        let rest = &header[search..];
        if rest.starts_with('*') {
            continue;
        }
        let Some(rest) = rest.trim_start().strip_prefix('=') else { continue };
        let rest = rest.trim_start();
        let value = if let Some(stripped) = rest.strip_prefix('"') {
            stripped.split('"').next().unwrap_or("")
        } else {
            rest.split(';').next().unwrap_or("").trim()
        };
        if !value.is_empty() {
            return Some(sanitize_file_name(value));
        }
    }
    None
}

/// URL yolunun son bölümünden dosya adı üretir.
pub fn file_name_from_url(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    let seg = parsed.path_segments()?.rfind(|s| !s.is_empty())?.to_string();
    let decoded = percent_decode_str(&seg).decode_utf8_lossy().to_string();
    let name = sanitize_file_name(&decoded);
    if name.is_empty() { None } else { Some(name) }
}

/// Yol ayracı, kontrol karakteri ve dosya sisteminde sorun çıkaran karakterleri temizler.
/// Windows'ta ayrıca o sistemin yasak karakter ve ad kuralları uygulanır.
pub fn sanitize_file_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let cleaned: String = base
        .chars()
        .map(|c| if c.is_control() || c == '\0' { '_' } else { c })
        .collect();
    let cleaned = cleaned.trim().trim_start_matches('.').to_string();
    let cleaned = if cfg!(windows) { windows_safe_name(&cleaned) } else { cleaned };
    // Dosya sistemi sınırı 255 bayt: uzantıyı koruyarak kısalt.
    if cleaned.len() <= 200 {
        return cleaned;
    }
    let (stem, ext) = match cleaned.rsplit_once('.') {
        Some((s, e)) if e.len() <= 8 => (s.to_string(), format!(".{e}")),
        _ => (cleaned.clone(), String::new()),
    };
    let mut cut = 190usize.saturating_sub(ext.len());
    while !stem.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}{}", &stem[..cut], ext)
}

/// Windows dosya adı kuralları (her platformda derlenir ki Linux'ta da sınanabilsin):
/// * `< > : " | ? *` kullanılamaz. `:` okunur kalsın diye `" - "`/`"-"` olur, diğerleri `_`/`'`.
/// * Ad nokta ya da boşlukla bitemez.
/// * `CON`, `PRN`, `AUX`, `NUL`, `COM1`–`COM9`, `LPT1`–`LPT9` uzantılı da olsa ayrılmış addır.
pub fn windows_safe_name(name: &str) -> String {
    let s = name.replace(": ", " - ");
    let s: String = s
        .chars()
        .map(|c| match c {
            ':' => '-',
            '"' => '\'',
            '<' | '>' | '|' | '?' | '*' => '_',
            c => c,
        })
        .collect();
    let s = s.trim_end_matches(['.', ' ']).to_string();
    let stem = s.split('.').next().unwrap_or("").trim_end();
    let reserved = {
        let u = stem.to_ascii_uppercase();
        matches!(u.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || ((u.starts_with("COM") || u.starts_with("LPT")) && u.len() == 4 && u.as_bytes()[3].is_ascii_digit() && u.as_bytes()[3] != b'0')
    };
    if reserved { format!("_{s}") } else { s }
}

/// Sayfa/video başlığını dosya adına çevirir. `sanitize_file_name`'den farkı: `/` ve `\` yol ayracı
/// sayılıp başlığın başı atılmaz, tireye çevrilir ("A/B" → "A-B").
pub fn sanitize_title(title: &str) -> String {
    sanitize_file_name(&title.replace(['/', '\\'], "-"))
}

/// Var olan bir dosyanın üzerine yazmamak için `ad (1).uzantı` biçiminde benzersiz ad bulur.
/// `.nehirpart` dosyaları da çakışma sayılır.
pub fn unique_file_name(dir: &Path, name: &str) -> String {
    let work = dir.join(crate::model::WORK_DIR_NAME);
    let taken = |n: &str| dir.join(n).exists() || work.join(format!("{n}.nehirpart")).exists();
    if !taken(name) {
        return name.to_string();
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() && e.len() <= 8 => (s, format!(".{e}")),
        _ => (name, String::new()),
    };
    // .tar.gz gibi çift uzantıyı koru
    let (stem, ext) = if let Some(s) = stem.strip_suffix(".tar") { (s, format!(".tar{ext}")) } else { (stem, ext) };
    for i in 1..10_000 {
        let candidate = format!("{stem} ({i}){ext}");
        if !taken(&candidate) {
            return candidate;
        }
    }
    format!("{stem}-{}{ext}", uuid::Uuid::new_v4().simple())
}

/// yt-dlp'nin dosya adını insan okunur hale getirir: sondaki ` [video-id]` kalkar, yt-dlp'nin
/// kullandığı tam genişlikli karakterler (｜ ＂ ： …) sade karşılıklarına çevrilir.
pub fn clean_video_name(name: &str) -> String {
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() && e.len() <= 5 => (s, format!(".{e}")),
        _ => (name, String::new()),
    };
    let id_suffix = regex::Regex::new(r"\s*\[[A-Za-z0-9_-]{6,}\]$").unwrap();
    let mut s = id_suffix.replace(stem, "").to_string();
    for (from, to) in [('｜', "-"), ('＂', ""), ('：', "-"), ('／', "-"), ('＼', "-"), ('＊', ""), ('？', ""), ('＜', ""), ('＞', "")] {
        s = s.replace(from, to);
    }
    let s = regex::Regex::new(r"\s+").unwrap().replace_all(&s, " ").trim().trim_matches('-').trim().to_string();
    if s.is_empty() { name.to_string() } else { format!("{s}{ext}") }
}

/// Bilgisayarın dili: "tr" ya da "en". LC_ALL → LC_MESSAGES → LANGUAGE → LANG sırasıyla bakılır;
/// Türkçe değilse İngilizce kabul edilir.
pub fn system_language() -> &'static str {
    detect_language(crate::platform::locale_var)
}

/// Dil algılama (test edilebilsin diye ortam değişkenleri dışarıdan verilir).
/// Önce arayüz dili (LC_ALL, LC_MESSAGES, LANGUAGE, LANG). Bunlar Türkçe değilse ama bölgesel biçimler
/// (saat, sayı, para birimi, kağıt) Türkçe ise kullanıcı Türkçe konuşuyor sayılır: İngilizce arayüz
/// kurulu Türk kullanıcılarda tipik durum budur.
pub fn detect_language(get: impl Fn(&str) -> Option<String>) -> &'static str {
    let norm = |k: &str| get(k).map(|v| v.trim().to_ascii_lowercase()).filter(|v| !v.is_empty() && v != "c" && v != "posix");
    for var in ["LC_ALL", "LC_MESSAGES", "LANGUAGE", "LANG"] {
        if let Some(v) = norm(var) {
            if v.starts_with("tr") {
                return "tr";
            }
            break;
        }
    }
    for var in ["LC_TIME", "LC_NUMERIC", "LC_MONETARY", "LC_PAPER", "LC_MEASUREMENT"] {
        if norm(var).map(|v| v.starts_with("tr")).unwrap_or(false) {
            return "tr";
        }
    }
    "en"
}

pub fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `~/Downloads` (XDG'ye saygılı), yoksa ev dizini.
pub fn default_download_dir() -> PathBuf {
    dirs::download_dir().or_else(dirs::home_dir).unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_disposition_variants() {
        assert_eq!(file_name_from_content_disposition("attachment; filename=\"a b.zip\"").as_deref(), Some("a b.zip"));
        assert_eq!(file_name_from_content_disposition("attachment; filename=a.zip").as_deref(), Some("a.zip"));
        assert_eq!(
            file_name_from_content_disposition("attachment; filename*=UTF-8''%C3%A7ay%20demi.pdf").as_deref(),
            Some("çay demi.pdf")
        );
        assert_eq!(
            file_name_from_content_disposition("attachment; filename=\"x.txt\"; filename*=UTF-8''y%20z.txt").as_deref(),
            Some("y z.txt")
        );
        assert_eq!(file_name_from_content_disposition("inline"), None);
    }

    #[test]
    fn sanitizes_traversal() {
        assert_eq!(sanitize_file_name("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_file_name("..\\..\\evil.exe"), "evil.exe");
        assert_eq!(sanitize_file_name(".bashrc"), "bashrc");
        assert!(sanitize_file_name(&"a".repeat(400)).len() < 255);
    }

    #[test]
    fn windows_names_are_made_safe() {
        assert_eq!(windows_safe_name("Güzel: Bir-Video.mp4"), "Güzel - Bir-Video.mp4");
        assert_eq!(windows_safe_name("12:30 raporu.pdf"), "12-30 raporu.pdf");
        assert_eq!(windows_safe_name("a<b>c|d?e*f\"g.txt"), "a_b_c_d_e_f'g.txt");
        assert_eq!(windows_safe_name("rapor.txt. ."), "rapor.txt");
        assert_eq!(windows_safe_name("CON"), "_CON");
        assert_eq!(windows_safe_name("nul.txt"), "_nul.txt");
        assert_eq!(windows_safe_name("com3.log"), "_com3.log");
        assert_eq!(windows_safe_name("LPT9"), "_LPT9");
        // Ayrılmış değiller:
        assert_eq!(windows_safe_name("COM0.txt"), "COM0.txt");
        assert_eq!(windows_safe_name("console.txt"), "console.txt");
        assert_eq!(windows_safe_name("nullable"), "nullable");
    }

    #[test]
    fn video_names_are_cleaned() {
        assert_eq!(clean_video_name("＂SARAÇHANE＂ ｜ Yunus'un Odası [9DsZySPCLL0].mp4"), "SARAÇHANE - Yunus'un Odası.mp4");
        assert_eq!(clean_video_name("Küsmek ｜ CM101MMXI FUNDAMENTALS [mIX0XgAbc].mp4"), "Küsmek - CM101MMXI FUNDAMENTALS.mp4");
        assert_eq!(clean_video_name("Normal Video.mp4"), "Normal Video.mp4");
        assert_eq!(clean_video_name("Video [ab].mp4"), "Video [ab].mp4"); // kısa köşeli ifade kimlik değil
    }

    #[test]
    fn language_detection() {
        let env = |pairs: &'static [(&'static str, &'static str)]| move |k: &str| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string());
        assert_eq!(detect_language(env(&[("LANG", "tr_TR.UTF-8")])), "tr");
        assert_eq!(detect_language(env(&[("LANG", "en_US.UTF-8")])), "en");
        // İngilizce arayüz + Türkiye bölge ayarları → Türkçe
        assert_eq!(detect_language(env(&[("LANG", "en_US.UTF-8"), ("LC_TIME", "tr_TR.UTF-8")])), "tr");
        assert_eq!(detect_language(env(&[("LANG", "de_DE.UTF-8"), ("LC_TIME", "de_DE.UTF-8")])), "en");
        assert_eq!(detect_language(env(&[("LANG", "C")])), "en");
        assert_eq!(detect_language(env(&[])), "en");
    }

    #[test]
    fn titles_keep_text_around_slashes() {
        assert_eq!(sanitize_title("Güzel: Bir/Video"), if cfg!(windows) { "Güzel - Bir-Video" } else { "Güzel: Bir-Video" });
        assert_eq!(sanitize_title("a\\b"), "a-b");
    }

    #[test]
    fn url_names() {
        assert_eq!(file_name_from_url("https://x.com/a/b/my%20file.iso?x=1").as_deref(), Some("my file.iso"));
        assert_eq!(file_name_from_url("https://x.com/"), None);
    }

    #[test]
    fn unique_names() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(unique_file_name(d.path(), "a.zip"), "a.zip");
        std::fs::write(d.path().join("a.zip"), b"x").unwrap();
        assert_eq!(unique_file_name(d.path(), "a.zip"), "a (1).zip");
        std::fs::write(d.path().join("a (1).zip"), b"x").unwrap();
        assert_eq!(unique_file_name(d.path(), "a.zip"), "a (2).zip");
    }
}
