//! yt-dlp sarmalayıcısı: sayfadaki videoları, m3u8/mpd akışlarını ve yt-dlp'nin desteklediği
//! 1000+ siteyi indirir; ffmpeg ile ses/görüntüyü birleştirir.

use crate::model::CookieEntry;
use crate::settings::Paths;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, Default)]
pub struct VideoProgress {
    pub downloaded: u64,
    pub total: i64,
    pub speed: f64,
}

pub struct VideoOpts {
    pub url: String,
    pub out_dir: PathBuf,
    pub quality: String,
    pub user_agent: Option<String>,
    pub referer: Option<String>,
    pub cookie_file: Option<PathBuf>,
    pub ytdlp: PathBuf,
    pub connections: u32,
    /// Ara dosyaların (.part, .f616.mp4, .f251.webm) tutulduğu gizli klasör. Bitmiş dosya `out_dir`'e taşınır.
    pub work_dir: PathBuf,
    /// Verilirse çıktı dosya adı bu gövdeden üretilir (uzantı yt-dlp'den gelir); yoksa video başlığı.
    pub name_stem: Option<String>,
}

use crate::platform::{exe_name, is_executable as is_exec};

fn which(name: &str) -> Option<PathBuf> {
    let name = exe_name(name);
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(&name)).find(|p| is_exec(p))
}

pub fn locate_ytdlp(custom: &str, paths: &Paths) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if !custom.is_empty() {
        candidates.push(PathBuf::from(custom));
    }
    candidates.push(paths.bin_dir().join(exe_name("yt-dlp")));
    #[cfg(unix)]
    {
        if let Some(home) = dirs::home_dir() {
            candidates.push(home.join(".local/bin/yt-dlp"));
        }
        candidates.push("/usr/local/bin/yt-dlp".into());
        candidates.push("/usr/bin/yt-dlp".into());
    }
    candidates.into_iter().find(|p| is_exec(p)).or_else(|| which("yt-dlp"))
}

pub fn locate_ffmpeg(paths: &Paths) -> Option<PathBuf> {
    let own = paths.bin_dir().join(exe_name("ffmpeg"));
    if is_exec(&own) { Some(own) } else { which("ffmpeg") }
}

/// YouTube'un imza/n-challenge'ını çözmek için yt-dlp'nin kullandığı JS çalışma ortamı.
pub fn locate_deno(paths: &Paths) -> Option<PathBuf> {
    let own = paths.bin_dir().join(exe_name("deno"));
    if is_exec(&own) {
        return Some(own);
    }
    which("deno").or_else(|| dirs::home_dir().map(|h| h.join(".deno").join("bin").join(exe_name("deno"))).filter(|p| is_exec(p)))
}

/// Eklentiden gelen çerezleri yt-dlp'nin okuduğu Netscape biçiminde yazar (0600).
pub fn write_cookie_file(path: &Path, cookies: &[CookieEntry]) -> std::io::Result<()> {
    let mut out = String::from("# Netscape HTTP Cookie File\n");
    for c in cookies {
        if c.name.contains(['\t', '\n']) || c.value.contains(['\t', '\n']) {
            continue;
        }
        let domain = if c.domain.is_empty() { continue } else { c.domain.as_str() };
        let include_sub = if domain.starts_with('.') { "TRUE" } else { "FALSE" };
        let path_s = if c.path.is_empty() { "/" } else { c.path.as_str() };
        out.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            domain,
            include_sub,
            path_s,
            if c.secure { "TRUE" } else { "FALSE" },
            c.expiration_date as u64,
            c.name,
            c.value
        ));
    }
    crate::settings::write_atomic(path, out.as_bytes(), 0o600)
}

fn format_args(quality: &str) -> Vec<String> {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    match quality {
        "audio" => s(&["-f", "bestaudio/best", "-x", "--audio-format", "mp3", "--audio-quality", "0"]),
        "best" | "" => s(&["-f", "bv*+ba/b", "--merge-output-format", "mp4"]),
        h if h.chars().all(|c| c.is_ascii_digit()) => vec![
            "-f".into(),
            format!("bv*[height<={h}]+ba/b[height<={h}]/b"),
            "--merge-output-format".into(),
            "mp4".into(),
        ],
        _ => s(&["-f", "bv*+ba/b", "--merge-output-format", "mp4"]),
    }
}

/// yt-dlp çıktısındaki bir satırı aşama koduna çevirir (arayüz yerelleştirir).
pub fn stage_for(line: &str) -> Option<String> {
    if let Some(name) = line.strip_prefix("[download] Destination: ") {
        let lower = name.to_ascii_lowercase();
        let re_audio = regex::Regex::new(r"\.f[0-9a-z\-]+\.(m4a|webm|opus|mp3|aac|ogg)$").unwrap();
        let re_part = regex::Regex::new(r"\.f[0-9a-z\-]+\.[a-z0-9]+$").unwrap();
        return Some(if re_audio.is_match(&lower) {
            "audio"
        } else if re_part.is_match(&lower) {
            "video"
        } else {
            "downloading"
        }
        .into());
    }
    if line.starts_with("[Merger]") {
        return Some("merging".into());
    }
    if line.starts_with("[VideoRemuxer]") || line.starts_with("[VideoConvertor]") {
        return Some("converting".into());
    }
    if line.starts_with("[ExtractAudio]") {
        return Some("extracting_audio".into());
    }
    if line.contains("has already been downloaded") {
        return Some("already_downloaded".into());
    }
    if let Some(title) = line.strip_prefix("NEHIRTITLE|") {
        let title = title.trim();
        return (!title.is_empty() && title != "NA").then(|| format!("title:{title}"));
    }
    if let Some(rest) = line.strip_prefix("NEHIRFMT|") {
        let p: Vec<&str> = rest.split('|').collect();
        if p.len() >= 5 {
            let mut parts = Vec::new();
            if p[0] != "NA" && !p[0].is_empty() {
                parts.push(p[0].to_string());
            }
            if p[1] != "NA" {
                parts.push(p[1].to_uppercase());
            }
            if p[2] != "NA" && p[2] != "none" {
                parts.push(p[2].split('.').next().unwrap_or("").to_string());
            }
            return Some(format!("format:{}", parts.join(" · ")));
        }
    }
    None
}

fn parse_progress(line: &str) -> Option<VideoProgress> {
    let rest = line.strip_prefix("NEHIRP|")?;
    let p: Vec<&str> = rest.split('|').collect();
    if p.len() < 4 {
        return None;
    }
    let num = |s: &str| s.parse::<f64>().ok();
    let downloaded = num(p[0]).unwrap_or(0.0).max(0.0) as u64;
    let total = num(p[1]).or_else(|| num(p[2])).map(|t| t as i64).unwrap_or(-1);
    Some(VideoProgress { downloaded, total, speed: num(p[3]).unwrap_or(0.0).max(0.0) })
}

/// yt-dlp'yi çalıştırır; bitince oluşan dosyanın yolunu döndürür.
pub async fn run(
    opts: VideoOpts,
    paths: &Paths,
    cancel: CancellationToken,
    on_progress: impl Fn(VideoProgress),
    on_stage: impl Fn(String),
) -> Result<Option<PathBuf>, String> {
    tokio::fs::create_dir_all(&opts.out_dir).await.map_err(|e| e.to_string())?;
    tokio::fs::create_dir_all(&opts.work_dir).await.map_err(|e| e.to_string())?;
    let template = match &opts.name_stem {
        Some(stem) => format!("{}.%(ext)s", stem.replace('%', "%%")),
        None => "%(title).150B [%(id)s].%(ext)s".to_string(),
    };
    let mut args: Vec<String> = vec![
        "--newline".into(), "--no-playlist".into(), "--no-colors".into(), "--progress".into(),
        "--no-simulate".into(), "--no-mtime".into(), "--no-quiet".into(), "--no-warnings".into(),
        "-N".into(), opts.connections.clamp(1, 8).to_string(),
        "-P".into(), format!("home:{}", opts.out_dir.display()),
        "-P".into(), format!("temp:{}", opts.work_dir.display()),
        "-o".into(), template,
        "--progress-template".into(),
        "download:NEHIRP|%(progress.downloaded_bytes)s|%(progress.total_bytes)s|%(progress.total_bytes_estimate)s|%(progress.speed)s".into(),
        "--print".into(), "after_move:filepath".into(),
        "--print".into(), "before_dl:NEHIRTITLE|%(title)s".into(),
        "--print".into(), "before_dl:NEHIRFMT|%(resolution)s|%(ext)s|%(vcodec)s|%(acodec)s|%(filesize_approx)s".into(),
    ];
    args.extend(format_args(&opts.quality));
    if let Some(ff) = locate_ffmpeg(paths) {
        args.extend(["--ffmpeg-location".into(), ff.to_string_lossy().into_owned()]);
    }
    if let Some(ua) = opts.user_agent.as_deref().filter(|s| !s.is_empty()) {
        args.extend(["--user-agent".into(), ua.into()]);
    }
    if let Some(r) = opts.referer.as_deref().filter(|s| !s.is_empty()) {
        args.extend(["--referer".into(), r.into()]);
    }
    if let Some(c) = &opts.cookie_file {
        args.extend(["--cookies".into(), c.to_string_lossy().into_owned()]);
    }
    if let Some(d) = locate_deno(paths) {
        args.extend(["--js-runtimes".into(), format!("deno:{}", d.display())]);
    }
    args.push("--".into());
    args.push(opts.url.clone());

    let mut cmd = Command::new(&opts.ytdlp);
    cmd.args(&args)
        .current_dir(&opts.work_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("PYTHONIOENCODING", "utf-8")
        .kill_on_drop(true);
    crate::platform::isolate_command(&mut cmd);
    let mut path_env = vec![paths.bin_dir()];
    if let Some(p) = std::env::var_os("PATH") {
        path_env.extend(std::env::split_paths(&p));
    }
    if let Ok(joined) = std::env::join_paths(path_env) {
        cmd.env("PATH", joined);
    }

    let mut child = cmd.spawn().map_err(|e| format!("yt-dlp başlatılamadı: {e}"))?;
    let tree = crate::platform::ProcessTree::attach(&child);
    let mut out = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut err = BufReader::new(child.stderr.take().unwrap()).lines();
    let mut final_path: Option<PathBuf> = None;
    let mut last_error: Option<String> = None;
    let (mut out_open, mut err_open) = (true, true);
    // yt-dlp'nin başlattığı alt süreçler (YouTube için deno) boruları açık tutabilir; bu yüzden
    // "borular kapanana kadar" değil, süreç çıkışını izler ve çıkıştan sonra kalan çıktıyı kısa süre boşaltırız.
    let mut exit_status: Option<std::process::ExitStatus> = None;
    let mut drain_until: Option<tokio::time::Instant> = None;

    loop {
        let drain_deadline = drain_until;
        tokio::select! {
            _ = cancel.cancelled() => {
                tree.kill();
                let _ = child.wait().await;
                return Ok(None);
            }
            st = child.wait(), if exit_status.is_none() => {
                exit_status = Some(st.map_err(|e| e.to_string())?);
                drain_until = Some(tokio::time::Instant::now() + std::time::Duration::from_millis(1500));
            }
            _ = async { tokio::time::sleep_until(drain_deadline.unwrap()).await }, if drain_deadline.is_some() => break,
            line = out.next_line(), if out_open => match line {
                Ok(Some(l)) => {
                    let l = l.trim().to_string();
                    if let Some(p) = parse_progress(&l) { on_progress(p); }
                    else if Path::new(&l).is_absolute() && Path::new(&l).exists() { final_path = Some(PathBuf::from(&l)); }
                    else if let Some(st) = stage_for(&l) { on_stage(st); }
                }
                _ => out_open = false,
            },
            line = err.next_line(), if err_open => match line {
                Ok(Some(l)) => {
                    let l = l.trim().to_string();
                    if l.starts_with("ERROR") { last_error = Some(l.clone()); }
                    if let Some(st) = stage_for(&l) { on_stage(st); }
                }
                _ => err_open = false,
            },
            else => break,
        }
        if exit_status.is_some() && !out_open && !err_open {
            break;
        }
    }
    // Geride kalan alt süreçleri (deno vb.) temizle.
    tree.kill();
    let status = match exit_status {
        Some(s) => s,
        None => child.wait().await.map_err(|e| e.to_string())?,
    };
    if status.success() {
        Ok(final_path)
    } else {
        Err(last_error.unwrap_or_else(|| format!("yt-dlp {} koduyla çıktı", status.code().unwrap_or(-1))))
    }
}

async fn download_bytes(client: &reqwest::Client, url: &str) -> Result<Vec<u8>, String> {
    let r = client.get(url).send().await.and_then(|r| r.error_for_status()).map_err(|e| e.to_string())?;
    r.bytes().await.map(|b| b.to_vec()).map_err(|e| e.to_string())
}

async fn install_binary(paths: &Paths, name: &str, data: Vec<u8>) -> Result<PathBuf, String> {
    let dir = paths.bin_dir();
    tokio::fs::create_dir_all(&dir).await.map_err(|e| e.to_string())?;
    let file = exe_name(name);
    let target = dir.join(&file);
    let tmp = dir.join(format!("{file}.tmp"));
    tokio::fs::write(&tmp, &data).await.map_err(|e| e.to_string())?;
    crate::platform::set_unix_mode(&tmp, 0o755).map_err(|e| e.to_string())?;
    tokio::fs::rename(&tmp, &target).await.map_err(|e| e.to_string())?;
    Ok(target)
}

#[cfg(windows)]
const YTDLP_URL: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe";
#[cfg(not(windows))]
const YTDLP_URL: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_linux";

#[cfg(windows)]
const DENO_URL: &str = "https://github.com/denoland/deno/releases/latest/download/deno-x86_64-pc-windows-msvc.zip";
#[cfg(not(windows))]
const DENO_URL: &str = "https://github.com/denoland/deno/releases/latest/download/deno-x86_64-unknown-linux-gnu.zip";

/// yt-dlp'nin bağımsız ikilisini (Linux'ta `yt-dlp_linux`, Windows'ta `yt-dlp.exe`) kullanıcı dizinine
/// indirir (kullanıcı isteğiyle). YouTube için gereken `deno` JS çalışma ortamı da (yoksa) birlikte kurulur.
pub async fn install_ytdlp(client: &reqwest::Client, paths: &Paths) -> Result<PathBuf, String> {
    let bytes = download_bytes(client, YTDLP_URL).await?;
    if bytes.len() < 1_000_000 {
        return Err("İndirilen yt-dlp beklenenden küçük".into());
    }
    let target = install_binary(paths, "yt-dlp", bytes).await?;
    if locate_deno(paths).is_none() {
        // deno olmadan YouTube bazı biçimleri vermez; hata yt-dlp kurulumunu engellemesin.
        if let Err(e) = install_deno(client, paths).await {
            tracing::warn!("deno kurulamadı: {e}");
        }
    }
    #[cfg(windows)]
    if locate_ffmpeg(paths).is_none() {
        // ffmpeg olmadan yt-dlp yalnızca tek dosyalı (düşük çözünürlüklü) biçimleri indirebilir; yine de engel olmasın.
        if let Err(e) = install_ffmpeg(client, paths).await {
            tracing::warn!("ffmpeg kurulamadı: {e}");
        }
    }
    Ok(target)
}

/// Zip içinden, yolu `suffix` ile biten ilk dosyanın içeriğini okur (ör. `bin/ffmpeg.exe` → `…/bin/ffmpeg.exe`).
/// Dizin adındaki sürüm/derleme etiketi değiştiği için tam yol yerine sonek aranır.
fn read_zip_entry(zip_bytes: &[u8], suffix: &str) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(zip_bytes)).map_err(|e| e.to_string())?;
    let idx = (0..z.len())
        .find(|&i| z.name_for_index(i).map(|n| n == suffix || n.ends_with(&format!("/{suffix}"))).unwrap_or(false))
        .ok_or_else(|| format!("zip içinde {suffix} yok"))?;
    let mut f = z.by_index(idx).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    f.read_to_end(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

async fn install_deno(client: &reqwest::Client, paths: &Paths) -> Result<(), String> {
    let zip_bytes = download_bytes(client, DENO_URL).await?;
    let name = exe_name("deno");
    let bin = tokio::task::spawn_blocking(move || read_zip_entry(&zip_bytes, &name)).await.map_err(|e| e.to_string())??;
    if bin.len() < 1_000_000 {
        return Err("deno beklenenden küçük".into());
    }
    install_binary(paths, "deno", bin).await.map(|_| ())
}

/// Windows: yt-dlp'nin önerdiği ffmpeg derlemesi. Linux'ta ffmpeg'i paket yöneticisi sağlar (`apt install ffmpeg`);
/// Windows'ta yoktur ve ffmpeg olmadan ses/görüntü birleştirilemez (yalnızca düşük çözünürlük iner).
#[cfg(windows)]
const FFMPEG_URL: &str = "https://github.com/yt-dlp/FFmpeg-Builds/releases/latest/download/ffmpeg-master-latest-win64-gpl.zip";

#[cfg(windows)]
async fn install_ffmpeg(client: &reqwest::Client, paths: &Paths) -> Result<(), String> {
    let zip_bytes = download_bytes(client, FFMPEG_URL).await?;
    for name in ["ffmpeg", "ffprobe"] {
        let (zb, entry) = (zip_bytes.clone(), format!("bin/{}", exe_name(name)));
        let bin = tokio::task::spawn_blocking(move || read_zip_entry(&zb, &entry)).await.map_err(|e| e.to_string())??;
        if bin.len() < 1_000_000 {
            return Err(format!("{name} beklenenden küçük"));
        }
        install_binary(paths, name, bin).await?;
    }
    Ok(())
}

/// yt-dlp hatası ağ kaynaklı ve sonra düzelebilir mi? (Desteklenmeyen adres, 4xx, giriş/DRM gibi kalıcı hatalar değil.)
pub fn is_transient_error(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    const PERMANENT: &[&str] = &["unsupported url", "http error 4", "private video", "sign in", "log in", "login", "drm", "not available", "removed", "copyright", "age-restricted", "members-only"];
    if PERMANENT.iter().any(|p| m.contains(p)) {
        return false;
    }
    const NETWORK: &[&str] = &[
        "timed out", "timeout", "connection reset", "connection aborted", "connection refused", "temporary failure", "name or service not known",
        "network is unreachable", "unable to download", "http error 5", "incomplete read", "remote end closed", "eof occurred", "getaddrinfo",
        "ssl", "transporterror", "read error", "bad gateway", "service unavailable",
    ];
    NETWORK.iter().any(|p| m.contains(p))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct FormatOption {
    pub height: u32,
    pub size: Option<u64>,
    pub ext: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct FormatList {
    pub title: String,
    pub duration: Option<f64>,
    pub options: Vec<FormatOption>,
}

/// `yt-dlp -J` çıktısından çözünürlük seçeneklerini çıkarır (en yüksek çözünürlük önce).
/// Boyut = o çözünürlükteki en büyük görüntü + (görüntü sessizse) en iyi ses.
pub fn parse_formats(info: &serde_json::Value) -> FormatList {
    let size_of = |f: &serde_json::Value| f.get("filesize").and_then(|v| v.as_u64()).or_else(|| f.get("filesize_approx").and_then(|v| v.as_f64()).map(|v| v as u64));
    let formats = info.get("formats").and_then(|f| f.as_array()).cloned().unwrap_or_default();
    let none = |f: &serde_json::Value, key: &str| f.get(key).and_then(|v| v.as_str()).map(|s| s == "none").unwrap_or(false);
    let best_audio = formats.iter().filter(|f| none(f, "vcodec") && !none(f, "acodec")).filter_map(size_of).max().unwrap_or(0);
    let mut by_height: std::collections::BTreeMap<u32, FormatOption> = std::collections::BTreeMap::new();
    for f in formats.iter().filter(|f| !none(f, "vcodec")) {
        let Some(h) = f.get("height").and_then(|v| v.as_u64()).map(|h| h as u32).filter(|h| *h > 0) else { continue };
        let mut size = size_of(f);
        if let (Some(s), true) = (size, none(f, "acodec")) {
            size = Some(s + best_audio);
        }
        let ext = f.get("ext").and_then(|v| v.as_str()).unwrap_or("mp4").to_string();
        let entry = by_height.entry(h).or_insert(FormatOption { height: h, size: None, ext: ext.clone() });
        if size > entry.size {
            entry.size = size;
        }
        if ext == "mp4" {
            entry.ext = ext;
        }
    }
    FormatList {
        title: info.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        duration: info.get("duration").and_then(|v| v.as_f64()),
        options: by_height.into_values().rev().collect(),
    }
}

/// Bir adresin kullanılabilir çözünürlüklerini yt-dlp'den sorar (indirmeden). En çok ~30 sn bekler.
pub async fn list_formats(
    ytdlp: &Path,
    paths: &Paths,
    url: &str,
    user_agent: Option<&str>,
    referer: Option<&str>,
    cookie_file: Option<&Path>,
) -> Result<FormatList, String> {
    let mut args: Vec<String> = vec!["-J".into(), "--no-playlist".into(), "--no-warnings".into(), "--skip-download".into(), "--no-colors".into()];
    if let Some(ua) = user_agent.filter(|s| !s.is_empty()) {
        args.extend(["--user-agent".into(), ua.into()]);
    }
    if let Some(r) = referer.filter(|s| !s.is_empty()) {
        args.extend(["--referer".into(), r.into()]);
    }
    if let Some(c) = cookie_file {
        args.extend(["--cookies".into(), c.to_string_lossy().into_owned()]);
    }
    if let Some(d) = locate_deno(paths) {
        args.extend(["--js-runtimes".into(), format!("deno:{}", d.display())]);
    }
    args.push("--".into());
    args.push(url.to_string());
    let mut cmd = Command::new(ytdlp);
    cmd.args(&args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true).env("PYTHONIOENCODING", "utf-8");
    crate::platform::isolate_command(&mut cmd);
    let child = cmd.spawn().map_err(|e| format!("yt-dlp başlatılamadı: {e}"))?;
    // Zaman aşımında future düşünce iş de kapanır; yt-dlp'nin alt süreçleri arkada kalmaz.
    let _tree = crate::platform::ProcessTree::attach(&child);
    let out = tokio::time::timeout(std::time::Duration::from_secs(30), child.wait_with_output())
        .await
        .map_err(|_| "zaman aşımı".to_string())?
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(err.lines().rev().find(|l| l.contains("ERROR")).unwrap_or("yt-dlp hata verdi").trim().to_string());
    }
    let info: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
    Ok(parse_formats(&info))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transient_error_classification() {
        let t = is_transient_error;
        assert!(t("ERROR: Unable to download webpage: <urlopen error [Errno -3] Temporary failure in name resolution>"));
        assert!(t("ERROR: unable to download video data: HTTP Error 503: Service Unavailable"));
        assert!(t("ERROR: [download] Got error: The read operation timed out"));
        assert!(!t("ERROR: Unsupported URL: https://www.netflix.com/watch/1"));
        assert!(!t("ERROR: [generic] 850x480.mp4: Unable to download webpage: HTTP Error 410: Gone"));
        assert!(!t("ERROR: Sign in to confirm you're not a bot"));
        assert!(!t("ERROR: Video unavailable. This video is not available"));
    }

    #[test]
    fn parses_format_list() {
        let info: serde_json::Value = serde_json::from_str(r#"{
          "title": "Örnek", "duration": 83.5,
          "formats": [
            {"format_id":"140","vcodec":"none","acodec":"mp4a","ext":"m4a","filesize":1000000},
            {"format_id":"137","vcodec":"avc1","acodec":"none","height":1080,"ext":"mp4","filesize":40000000},
            {"format_id":"299","vcodec":"avc1","acodec":"none","height":1080,"ext":"mp4","filesize":60000000},
            {"format_id":"136","vcodec":"avc1","acodec":"none","height":720,"ext":"webm","filesize_approx":20000000.0},
            {"format_id":"18","vcodec":"avc1","acodec":"mp4a","height":360,"ext":"mp4","filesize":5000000},
            {"format_id":"sb0","vcodec":"none","acodec":"none","ext":"mhtml"}
          ]}"#).unwrap();
        let l = parse_formats(&info);
        assert_eq!(l.title, "Örnek");
        assert_eq!(l.options.iter().map(|o| o.height).collect::<Vec<_>>(), vec![1080, 720, 360]);
        assert_eq!(l.options[0].size, Some(61_000_000)); // en büyük 1080p + ses
        assert_eq!(l.options[1].size, Some(21_000_000));
        assert_eq!(l.options[2].size, Some(5_000_000)); // sesli biçim: ek ses yok
    }

    fn zip_with(entries: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write;
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, data) in entries {
            w.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(data).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    #[test]
    fn finds_zip_entry_by_path_suffix() {
        let z = zip_with(&[
            ("ffmpeg-master-latest-win64-gpl/doc/readme.txt", b"doc"),
            ("ffmpeg-master-latest-win64-gpl/xbin/ffmpeg.exe", b"WRONG"),
            ("ffmpeg-master-latest-win64-gpl/bin/ffmpeg.exe", b"RIGHT"),
            ("deno.exe", b"DENO"),
        ]);
        // Sürüm dizininin adı ne olursa olsun `bin/ffmpeg.exe` bulunur; `xbin/` ile karışmaz.
        assert_eq!(read_zip_entry(&z, "bin/ffmpeg.exe").unwrap(), b"RIGHT");
        // Kökteki dosya (deno zip'i) tam adla bulunur.
        assert_eq!(read_zip_entry(&z, "deno.exe").unwrap(), b"DENO");
        assert!(read_zip_entry(&z, "bin/ffprobe.exe").is_err());
        assert!(read_zip_entry(b"not a zip", "x").is_err());
    }

    #[test]
    fn parses_progress_lines() {
        let p = parse_progress("NEHIRP|1048576|NA|2097152|524288.0").unwrap();
        assert_eq!(p.downloaded, 1_048_576);
        assert_eq!(p.total, 2_097_152);
        assert_eq!(p.speed, 524_288.0);
        assert!(parse_progress("random line").is_none());
    }

    #[test]
    fn maps_stages() {
        assert_eq!(stage_for("[download] Destination: /x/a.f140.m4a").as_deref(), Some("audio"));
        assert_eq!(stage_for("[download] Destination: /x/a.f137.mp4").as_deref(), Some("video"));
        assert_eq!(stage_for("[download] Destination: /x/a.mp4").as_deref(), Some("downloading"));
        assert_eq!(stage_for("[Merger] Merging formats into x").as_deref(), Some("merging"));
        assert_eq!(stage_for("NEHIRTITLE|Güzel Video").as_deref(), Some("title:Güzel Video"));
        assert_eq!(stage_for("NEHIRTITLE|NA"), None);
        assert_eq!(stage_for("NEHIRFMT|1920x1080|mp4|avc1.640028|mp4a|123").as_deref(), Some("format:1920x1080 · MP4 · avc1"));
    }

    #[test]
    fn cookie_file_format() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("c.txt");
        write_cookie_file(&f, &[CookieEntry { name: "a".into(), value: "b".into(), domain: ".x.com".into(), path: "/".into(), secure: true, expiration_date: 1700000000.5 }]).unwrap();
        let s = std::fs::read_to_string(f).unwrap();
        assert!(s.contains(".x.com\tTRUE\t/\tTRUE\t1700000000\ta\tb"));
    }
}
