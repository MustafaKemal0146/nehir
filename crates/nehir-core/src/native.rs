//! Tarayıcıların "native messaging" manifestlerini kaydeder.
//!
//! Eklenti uygulama kapalıyken `runtime.sendNativeMessage` ile `nehir-native-host`'u çağırır;
//! o da uygulamayı arka planda başlatır. URL/çerez gibi hiçbir veri bu yoldan geçmez.

use std::path::{Path, PathBuf};

pub const HOST_NAME: &str = "org.nehir.host";
/// Firefox eklenti kimliği (manifest.json `browser_specific_settings.gecko.id`).
pub const FIREFOX_ID: &str = "nehir@nehir.app";
/// Chrome eklentisinin sabit kimliği (extension/chrome-key.pub.b64'ten türetilir).
pub const CHROME_EXTENSION_ID: &str = "pmoiiopgfbkmmnkogefipgaiihmibdbb";

#[cfg(unix)]
struct Target {
    name: &'static str,
    /// `$HOME`'a göre klasör
    rel_dir: &'static str,
    firefox: bool,
}

#[cfg(unix)]
const TARGETS: &[Target] = &[
    Target { name: "Google Chrome", rel_dir: ".config/google-chrome/NativeMessagingHosts", firefox: false },
    Target { name: "Chromium", rel_dir: ".config/chromium/NativeMessagingHosts", firefox: false },
    Target { name: "Brave", rel_dir: ".config/BraveSoftware/Brave-Browser/NativeMessagingHosts", firefox: false },
    Target { name: "Microsoft Edge", rel_dir: ".config/microsoft-edge/NativeMessagingHosts", firefox: false },
    Target { name: "Vivaldi", rel_dir: ".config/vivaldi/NativeMessagingHosts", firefox: false },
    Target { name: "Opera", rel_dir: ".config/opera/NativeMessagingHosts", firefox: false },
    Target { name: "Firefox", rel_dir: ".mozilla/native-messaging-hosts", firefox: true },
];

pub fn manifest_json(host_binary: &Path, chromium_ids: &[String], firefox: bool) -> String {
    let mut v = serde_json::json!({
        "name": HOST_NAME,
        "description": "Nehir Download Manager",
        "path": host_binary,
        "type": "stdio",
    });
    if firefox {
        v["allowed_extensions"] = serde_json::json!([FIREFOX_ID]);
    } else {
        v["allowed_origins"] = chromium_ids.iter().map(|id| format!("chrome-extension://{id}/")).collect::<Vec<_>>().into();
    }
    serde_json::to_string_pretty(&v).unwrap()
}

pub struct Registered {
    pub browser: &'static str,
    pub path: PathBuf,
}

/// Kullanıcı dizinindeki tarayıcı klasörlerine manifest yazar. Yalnızca kurulu görünen
/// tarayıcılar için (üst klasörü var olanlara) yazar.
#[cfg(unix)]
pub fn register_for_user(home: &Path, host_binary: &Path, chromium_ids: &[String]) -> Vec<Registered> {
    let mut done = Vec::new();
    for t in TARGETS {
        let dir = home.join(t.rel_dir);
        let browser_root = dir.parent().map(Path::to_path_buf).unwrap_or_default();
        if !browser_root.exists() {
            continue;
        }
        if std::fs::create_dir_all(&dir).is_err() {
            continue;
        }
        let file = dir.join(format!("{HOST_NAME}.json"));
        if std::fs::write(&file, manifest_json(host_binary, chromium_ids, t.firefox)).is_ok() {
            done.push(Registered { browser: t.name, path: file });
        }
    }
    done
}

/// Windows: tarayıcılar manifest klasörüne bakmaz, kayıt defterine bakar. Her tarayıcının
/// `HKCU\<anahtar>\NativeMessagingHosts\org.nehir.host` girdisinin varsayılan değeri manifest dosyasının yoludur.
///
/// Vivaldi ve Opera Chromium tabanlıdır ve Chrome'un anahtarını da okur; ayrıca kayıt gerekmez.
#[cfg(windows)]
mod windows {
    use super::*;
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    struct WinTarget {
        name: &'static str,
        /// `HKCU` altında, `NativeMessagingHosts` anahtarının bulunduğu üst anahtar.
        key: &'static str,
        manifest: &'static str,
        firefox: bool,
    }

    const TARGETS: &[WinTarget] = &[
        WinTarget { name: "Google Chrome", key: r"Software\Google\Chrome", manifest: "chrome.json", firefox: false },
        WinTarget { name: "Microsoft Edge", key: r"Software\Microsoft\Edge", manifest: "chrome.json", firefox: false },
        WinTarget { name: "Brave", key: r"Software\BraveSoftware\Brave-Browser", manifest: "chrome.json", firefox: false },
        WinTarget { name: "Chromium", key: r"Software\Chromium", manifest: "chrome.json", firefox: false },
        WinTarget { name: "Firefox", key: r"Software\Mozilla", manifest: "firefox.json", firefox: true },
    ];

    fn host_key(t: &WinTarget, base: &str) -> String {
        let prefix = if base.is_empty() { String::new() } else { format!(r"{base}\") };
        format!(r"{prefix}{}\NativeMessagingHosts\{HOST_NAME}", t.key)
    }

    /// Manifestleri `manifest_dir` altına yazar ve her tarayıcının kayıt defteri anahtarını bunlara yöneltir.
    /// `base` boşsa gerçek konumlar kullanılır; testler ayrı bir alt anahtar verir.
    pub fn register_under(base: &str, manifest_dir: &Path, host_binary: &Path, chromium_ids: &[String]) -> Vec<Registered> {
        let mut done = Vec::new();
        if std::fs::create_dir_all(manifest_dir).is_err() {
            return done;
        }
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        for t in TARGETS {
            let file = manifest_dir.join(t.manifest);
            if std::fs::write(&file, manifest_json(host_binary, chromium_ids, t.firefox)).is_err() {
                continue;
            }
            let Ok((key, _)) = hkcu.create_subkey(host_key(t, base)) else { continue };
            if key.set_value("", &file.to_string_lossy().as_ref()).is_ok() {
                done.push(Registered { browser: t.name, path: file });
            }
        }
        done
    }

    /// Kaldırma: kayıt defteri girdilerini siler (manifest dosyaları klasörle birlikte silinir).
    pub fn unregister_under(base: &str) {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        for t in TARGETS {
            let _ = hkcu.delete_subkey_all(host_key(t, base));
        }
    }

    #[cfg(test)]
    pub fn registered_manifest(base: &str, browser: &str) -> Option<PathBuf> {
        let t = TARGETS.iter().find(|t| t.name == browser)?;
        let key = RegKey::predef(HKEY_CURRENT_USER).open_subkey(host_key(t, base)).ok()?;
        key.get_value::<String, _>("").ok().map(PathBuf::from)
    }
}

/// Kullanıcı için tüm desteklenen tarayıcıları kaydeder (Windows).
#[cfg(windows)]
pub fn register_for_user(manifest_dir: &Path, host_binary: &Path, chromium_ids: &[String]) -> Vec<Registered> {
    windows::register_under("", manifest_dir, host_binary, chromium_ids)
}

/// Kayıt defteri girdilerini siler (kaldırma sırasında).
#[cfg(windows)]
pub fn unregister_for_user() {
    windows::unregister_under("");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chrome_id_matches_repo_file() {
        let id = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../extension/chrome-extension-id.txt")).unwrap();
        assert_eq!(id.trim(), CHROME_EXTENSION_ID);
    }

    #[test]
    fn manifests_match_browser_family() {
        let c = manifest_json(Path::new("/usr/bin/nehir-native-host"), &["abc".into()], false);
        assert!(c.contains("chrome-extension://abc/") && !c.contains("allowed_extensions"));
        let f = manifest_json(Path::new("/usr/bin/nehir-native-host"), &[], true);
        assert!(f.contains(FIREFOX_ID) && !f.contains("allowed_origins"));
    }

    /// Gerçek tarayıcı anahtarlarına dokunmaz: her şey `HKCU\Software\NehirTest\<uuid>` altında kurulur.
    #[cfg(windows)]
    #[test]
    fn registers_in_registry_and_cleans_up() {
        let base = format!(r"Software\NehirTest\{}", uuid::Uuid::new_v4().simple());
        let dir = tempfile::tempdir().unwrap();
        let host = Path::new(r"C:\Program Files\Nehir\nehir-native-host.exe");
        let r = windows::register_under(&base, dir.path(), host, &[CHROME_EXTENSION_ID.into()]);
        assert_eq!(r.len(), 5, "Chrome, Edge, Brave, Chromium, Firefox");

        // Anahtarın değeri, içinde doğru host yolu olan manifest dosyasını göstermeli.
        let chrome = windows::registered_manifest(&base, "Google Chrome").unwrap();
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(chrome).unwrap()).unwrap();
        assert_eq!(v["name"], HOST_NAME);
        assert_eq!(v["path"], host.to_string_lossy().as_ref());
        assert_eq!(v["allowed_origins"][0], format!("chrome-extension://{CHROME_EXTENSION_ID}/"));
        let ff = windows::registered_manifest(&base, "Firefox").unwrap();
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(ff).unwrap()).unwrap();
        assert_eq!(v["allowed_extensions"][0], FIREFOX_ID);

        windows::unregister_under(&base);
        assert!(windows::registered_manifest(&base, "Google Chrome").is_none());
        // Test kökünü de sil.
        let _ = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER).delete_subkey_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn registers_only_installed_browsers() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".config/google-chrome")).unwrap();
        let r = register_for_user(home.path(), Path::new("/x/host"), &["id".into()]);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].browser, "Google Chrome");
        assert!(r[0].path.exists());
    }
}
