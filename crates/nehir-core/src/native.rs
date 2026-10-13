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

struct Target {
    name: &'static str,
    /// `$HOME`'a göre klasör
    rel_dir: &'static str,
    firefox: bool,
}

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
