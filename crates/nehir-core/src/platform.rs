//! İşletim sistemine bağlı işlerin tek yeri: konumlu yazma, disk yeri, süreç ağacı, dosya izinleri.
//!
//! Linux ve Windows kolları aynı imzayı taşır; çağıran kod `cfg` görmez.

use std::fs::File;
use std::io;
use std::path::Path;

// ───────────────────────────── Konumlu dosya yazma ─────────────────────────────

/// `buf`'ın tamamını `offset` konumuna yazar. Dosya imlecine bağlı değildir; aynı `File` birden çok
/// iş parçacığından farklı konumlara yazılabilir.
#[cfg(unix)]
pub fn write_all_at(f: &File, buf: &[u8], offset: u64) -> io::Result<()> {
    std::os::unix::fs::FileExt::write_all_at(f, buf, offset)
}

#[cfg(windows)]
pub fn write_all_at(f: &File, mut buf: &[u8], mut offset: u64) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    // `seek_write` dosya imlecini oynatır; hiçbir yerde imlece güvenilmediği için sorun olmaz.
    while !buf.is_empty() {
        match f.seek_write(buf, offset) {
            Ok(0) => return Err(io::Error::new(io::ErrorKind::WriteZero, "dosyaya yazılamadı")),
            Ok(n) => {
                buf = &buf[n..];
                offset += n as u64;
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

// ───────────────────────────── Disk yeri ─────────────────────────────

/// Klasörün bulunduğu diskte kullanılabilir boş yer (bayt).
#[cfg(unix)]
pub fn free_space(dir: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    Some(st.f_bavail as u64 * st.f_frsize as u64)
}

#[cfg(windows)]
pub fn free_space(dir: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let mut avail_to_caller = 0u64;
    let (mut total, mut free) = (0u64, 0u64);
    // Kullanıcının kotası varsa `avail_to_caller` onu yansıtır; gerçek kullanılabilir yer odur.
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut avail_to_caller, &mut total, &mut free) };
    if ok == 0 { None } else { Some(avail_to_caller) }
}

/// Ön ayırma sonucu: yer yetmedi mi, başka bir hata mı?
#[derive(Debug)]
pub enum PreallocError {
    NoSpace,
    Io(io::Error),
}

/// Dosyanın tüm yerini baştan ayırır. Dosya sistemi desteklemiyorsa boyutu ayarlamakla yetinir.
#[cfg(unix)]
pub fn preallocate(f: &File, total: u64) -> Result<(), PreallocError> {
    use std::os::fd::AsRawFd;
    if total == 0 {
        return Ok(());
    }
    let r = unsafe { libc::fallocate(f.as_raw_fd(), 0, 0, total as libc::off_t) };
    if r == 0 {
        return Ok(());
    }
    let e = io::Error::last_os_error();
    if e.raw_os_error() == Some(libc::ENOSPC) {
        return Err(PreallocError::NoSpace);
    }
    f.set_len(total).map_err(PreallocError::Io)
}

/// NTFS'te `SetEndOfFile` dosyanın kümelerini ayırır; disk doluysa burada hata verir.
#[cfg(windows)]
pub fn preallocate(f: &File, total: u64) -> Result<(), PreallocError> {
    if total == 0 {
        return Ok(());
    }
    match f.set_len(total) {
        Ok(()) => Ok(()),
        // ERROR_DISK_FULL (112), ERROR_HANDLE_DISK_FULL (39)
        Err(e) if matches!(e.raw_os_error(), Some(112) | Some(39)) => Err(PreallocError::NoSpace),
        Err(e) => Err(PreallocError::Io(e)),
    }
}

// ───────────────────────────── Dosya izinleri ve çalıştırılabilirlik ─────────────────────────────

/// Unix dosya modunu ayarlar (ör. 0o600 yalnızca sahibine açar). Windows'ta kullanıcı profili zaten sahibine özeldir;
/// ayrıca bir şey yapılmaz.
#[cfg(unix)]
pub fn set_unix_mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

#[cfg(windows)]
pub fn set_unix_mode(_path: &Path, _mode: u32) -> io::Result<()> {
    Ok(())
}

/// Çalıştırılabilir bir dosya mı?
#[cfg(unix)]
pub fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.is_file() && std::fs::metadata(p).map(|m| m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

#[cfg(windows)]
pub fn is_executable(p: &Path) -> bool {
    p.is_file()
        && p.extension()
            .and_then(|e| e.to_str())
            .map(|e| matches!(e.to_ascii_lowercase().as_str(), "exe" | "com" | "bat" | "cmd"))
            .unwrap_or(false)
}

/// Çalıştırılabilir dosya adı: Windows'ta `.exe` eklenir.
pub fn exe_name(name: &str) -> String {
    if cfg!(windows) { format!("{name}.exe") } else { name.to_string() }
}

/// Dosyanın gizli olarak işaretlenmesi. Unix'te baştaki nokta yeterlidir; Windows'ta nitelik gerekir.
#[cfg(unix)]
pub fn hide_path(_path: &Path) {}

#[cfg(windows)]
pub fn hide_path(path: &Path) {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{GetFileAttributesW, SetFileAttributesW, FILE_ATTRIBUTE_HIDDEN, INVALID_FILE_ATTRIBUTES};
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    unsafe {
        let attrs = GetFileAttributesW(wide.as_ptr());
        if attrs != INVALID_FILE_ATTRIBUTES {
            SetFileAttributesW(wide.as_ptr(), attrs | FILE_ATTRIBUTE_HIDDEN);
        }
    }
}

// ───────────────────────────── Süreç ağacı ─────────────────────────────

/// Alt süreç başlatılırken kendi süreç grubunda (Unix) / konsolsuz yeni grupta (Windows) çalıştırılır.
#[cfg(unix)]
pub fn isolate_command(cmd: &mut tokio::process::Command) -> &mut tokio::process::Command {
    cmd.process_group(0)
}

#[cfg(windows)]
pub fn isolate_command(cmd: &mut tokio::process::Command) -> &mut tokio::process::Command {
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW)
}

/// Bir alt sürecin ve onun başlattığı tüm alt süreçlerin (yt-dlp → deno, ffmpeg) topluca öldürülmesi.
///
/// Unix'te süreç grubu, Windows'ta Job Object kullanılır. Nesne düşünce (Windows'ta) iş içindeki
/// tüm süreçler de sonlandırılır.
pub struct ProcessTree {
    #[cfg(unix)]
    pid: Option<u32>,
    #[cfg(windows)]
    job: Option<win_job::Job>,
}

impl ProcessTree {
    /// `child` az önce başlatılmış olmalıdır.
    pub fn attach(child: &tokio::process::Child) -> ProcessTree {
        #[cfg(unix)]
        {
            ProcessTree { pid: child.id() }
        }
        #[cfg(windows)]
        {
            ProcessTree { job: win_job::Job::for_child(child) }
        }
    }

    pub fn kill(&self) {
        #[cfg(unix)]
        if let Some(pid) = self.pid {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGTERM);
            }
        }
        #[cfg(windows)]
        if let Some(j) = &self.job {
            j.terminate();
        }
    }
}

#[cfg(windows)]
mod win_job {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject, TerminateJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    pub struct Job(HANDLE);

    // HANDLE ham bir işaretçidir; Job nesnesi tek sahiplidir ve çekirdek nesnesi iş parçacığı güvenlidir.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    impl Job {
        pub fn for_child(child: &tokio::process::Child) -> Option<Job> {
            let proc = child.raw_handle()?;
            unsafe {
                let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if job.is_null() {
                    return None;
                }
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let ok = SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const _,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if ok == 0 || AssignProcessToJobObject(job, proc as HANDLE) == 0 {
                    CloseHandle(job);
                    return None;
                }
                Some(Job(job))
            }
        }

        pub fn terminate(&self) {
            unsafe {
                TerminateJobObject(self.0, 1);
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

// ───────────────────────────── Kabuk işlemleri ─────────────────────────────

fn quiet(cmd: &mut std::process::Command) -> &mut std::process::Command {
    use std::process::Stdio;
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd
}

/// Dosyayı ya da klasörü varsayılan uygulamayla açar (Linux: `xdg-open`, Windows: Gezgin).
pub fn open_path(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    let mut cmd = std::process::Command::new("xdg-open");
    #[cfg(windows)]
    let mut cmd = std::process::Command::new("explorer.exe");
    quiet(cmd.arg(path)).spawn().map(|_| ()).map_err(|e| format!("açılamadı: {e}"))
}

/// Dosyayı klasöründe gösterir: Windows'ta Gezgin dosyayı seçili açar, Linux'ta klasörü açar.
pub fn reveal_path(path: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        // `/select,<yol>` tek argüman olmalı; boşluklu yollar için Rust'ın tırnaklaması yeterlidir.
        let mut arg = std::ffi::OsString::from("/select,");
        arg.push(path);
        return std::process::Command::new("explorer.exe").arg(arg).spawn().map(|_| ()).map_err(|e| format!("açılamadı: {e}"));
    }
    #[cfg(unix)]
    open_path(path.parent().unwrap_or(path))
}

/// Bilgisayarı uykuya alır.
pub fn suspend() -> bool {
    #[cfg(unix)]
    return quiet(std::process::Command::new("systemctl").arg("suspend")).status().map(|s| s.success()).unwrap_or(false);
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Power::SetSuspendState;
        // bHibernate = FALSE: uyku (hazırda bekletme açık olsa bile hazırda beklemeye geçmez).
        return unsafe { SetSuspendState(false, false, false) };
    }
}

/// Bilgisayarı kapatır.
pub fn power_off() -> bool {
    #[cfg(unix)]
    return quiet(std::process::Command::new("systemctl").arg("poweroff")).status().map(|s| s.success()).unwrap_or(false);
    #[cfg(windows)]
    return quiet(std::process::Command::new("shutdown.exe").args(["/s", "/t", "0"])).status().map(|s| s.success()).unwrap_or(false);
}

/// Oturum açılınca başlatma (Windows: `HKCU\...\Run`). Linux'ta `.desktop` dosyası uygulama
/// tarafında yönetilir.
#[cfg(windows)]
pub fn set_run_at_login(name: &str, exe: &Path, on: bool) -> io::Result<()> {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_SET_VALUE};
    use winreg::RegKey;
    let run = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(r"Software\Microsoft\Windows\CurrentVersion\Run", KEY_SET_VALUE)?;
    if on {
        run.set_value(name, &format!("\"{}\" --background", exe.display()))
    } else {
        match run.delete_value(name) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            r => r,
        }
    }
}

/// İndirilen dosyaya "internetten geldi" işareti (Mark-of-the-Web) koyar; böylece Windows,
/// çalıştırılabilir dosyalarda SmartScreen/Defender uyarısını tarayıcı indirmeleri gibi gösterir.
/// NTFS dışı birimlerde sessizce atlanır. Unix'te bir şey yapmaz.
pub fn mark_downloaded(file: &Path, url: &str, referrer: Option<&str>) {
    #[cfg(windows)]
    {
        let mut body = String::from("[ZoneTransfer]\r\nZoneId=3\r\n");
        if let Some(r) = referrer.filter(|r| !r.is_empty()) {
            body.push_str(&format!("ReferrerUrl={r}\r\n"));
        }
        body.push_str(&format!("HostUrl={url}\r\n"));
        let mut ads = file.as_os_str().to_owned();
        ads.push(":Zone.Identifier");
        let _ = std::fs::write(std::path::PathBuf::from(ads), body);
    }
    #[cfg(unix)]
    let _ = (file, url, referrer);
}

// ───────────────────────────── Dil ─────────────────────────────

/// Windows'ta ortam değişkeni yerine sistem yerel ayarı okunur. `detect_language`'in beklediği
/// `LANG` (arayüz dili) ve `LC_TIME` (bölgesel biçim) anahtarlarına eşlenir.
#[cfg(windows)]
pub fn locale_var(key: &str) -> Option<String> {
    use windows_sys::Win32::Globalization::{GetUserDefaultLocaleName, GetUserDefaultUILanguage};
    match key {
        "LANG" => {
            // LANGID'in alt 10 biti birincil dildir; 0x1F = Türkçe.
            let id = unsafe { GetUserDefaultUILanguage() };
            Some(if id & 0x3ff == 0x1f { "tr_TR".to_string() } else { "other".to_string() })
        }
        "LC_TIME" => {
            let mut buf = [0u16; 85];
            let n = unsafe { GetUserDefaultLocaleName(buf.as_mut_ptr(), buf.len() as i32) };
            if n > 1 { Some(String::from_utf16_lossy(&buf[..(n as usize - 1)]).replace('-', "_")) } else { None }
        }
        _ => None,
    }
}

#[cfg(not(windows))]
pub fn locale_var(key: &str) -> Option<String> {
    std::env::var(key).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positional_writes_do_not_depend_on_cursor() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("f.bin");
        let f = std::fs::OpenOptions::new().create(true).read(true).write(true).truncate(true).open(&p).unwrap();
        preallocate(&f, 10).unwrap();
        write_all_at(&f, b"world", 5).unwrap();
        write_all_at(&f, b"hello", 0).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"helloworld");
    }

    #[test]
    fn free_space_is_reported() {
        let d = tempfile::tempdir().unwrap();
        assert!(free_space(d.path()).unwrap() > 0);
    }

    /// Üst süreç her saniye yeni bir alt süreç (ping / sleep) başlatıp dosyaya satır ekler. Ağaç
    /// öldürülünce hem üst süreç hem alt süreçler ölmeli; dosya artık büyümemeli.
    #[tokio::test]
    async fn killing_the_tree_stops_grandchildren_too() {
        let d = tempfile::tempdir().unwrap();
        let marker = d.path().join("marker.txt");
        #[cfg(windows)]
        let mut cmd = {
            // `cmd` (üst süreç) çıktısı işaret dosyasına yönlendirilmiş bir `ping` (torun) başlatır. Dosyaya yalnızca
            // torun yazar; yani üst süreç tek başına öldürülürse dosya büyümeye devam eder.
            let bat = d.path().join("loop.cmd");
            std::fs::write(&bat, "@echo off\r\nping -n 300 127.0.0.1 > \"%~dp0marker.txt\"\r\n").unwrap();
            tokio::process::Command::new(bat)
        };
        #[cfg(unix)]
        let mut cmd = {
            let mut c = tokio::process::Command::new("sh");
            c.args(["-c", &format!("(while true; do echo x >> '{}'; sleep 1; done) & wait", marker.display())]);
            c
        };
        cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).kill_on_drop(true);
        isolate_command(&mut cmd);
        let mut child = cmd.spawn().unwrap();
        let tree = ProcessTree::attach(&child);

        let size = || std::fs::metadata(&marker).map(|m| m.len()).unwrap_or(0);
        tokio::time::sleep(std::time::Duration::from_millis(3500)).await;
        assert!(size() > 0, "döngü çalışmıyor; test geçersiz");

        tree.kill();
        let status = tokio::time::timeout(std::time::Duration::from_secs(5), child.wait()).await.expect("üst süreç ölmedi").unwrap();
        assert!(!status.success());
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await; // yazılmakta olan son satır bitsin
        let after_kill = size();
        tokio::time::sleep(std::time::Duration::from_millis(3000)).await;
        assert_eq!(size(), after_kill, "ağaç öldürüldü ama alt süreçler yazmaya devam ediyor");
    }

    #[cfg(windows)]
    #[test]
    fn downloaded_files_get_mark_of_the_web() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("kurulum.exe");
        std::fs::write(&p, b"MZ").unwrap();
        mark_downloaded(&p, "https://example.com/kurulum.exe", Some("https://example.com/indir"));
        let mut ads = p.as_os_str().to_owned();
        ads.push(":Zone.Identifier");
        let s = std::fs::read_to_string(std::path::PathBuf::from(ads)).unwrap();
        assert!(s.contains("ZoneId=3") && s.contains("HostUrl=https://example.com/kurulum.exe") && s.contains("ReferrerUrl=https://example.com/indir"));
        // Asıl dosyanın içeriği değişmez.
        assert_eq!(std::fs::read(&p).unwrap(), b"MZ");
    }

    #[test]
    fn exe_names() {
        assert_eq!(exe_name("yt-dlp"), if cfg!(windows) { "yt-dlp.exe" } else { "yt-dlp" });
    }

    #[test]
    fn executable_detection() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join(exe_name("tool"));
        assert!(!is_executable(&p));
        std::fs::write(&p, b"x").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert!(is_executable(&p));
    }
}
