//! `packaging/nehir-nettune` yardımcısını sahte bir kök dizinde ve sahte `sysctl` ile sınar.
//! Gerçek sistem ayarlarına DOKUNMAZ (NEHIR_NETTUNE_ROOT ayarlıyken yönetici yetkisi de istenmez).

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn helper() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging/nehir-nettune")
}

/// Değerleri `<root>/state/<anahtar>` dosyalarında tutan sahte sysctl.
fn fake_sysctl(root: &Path) -> PathBuf {
    let p = root.join("fake-sysctl");
    fs::write(
        &p,
        r#"#!/bin/sh
S="$NEHIR_NETTUNE_ROOT/state"
mkdir -p "$S"
case "$1" in
  -n) [ -f "$S/$2" ] && cat "$S/$2" || exit 1 ;;
  -q)
    case "$2" in
      -w) kv="$3"; k=${kv%%=*}; v=${kv#*=}; printf '%s\n' "$v" > "$S/$k" ;;
      -p) while IFS= read -r l; do case "$l" in \#*|"") continue;; esac; k=${l%% = *}; v=${l#* = }; printf '%s\n' "$v" > "$S/$k"; done < "$3" ;;
    esac ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    p
}

struct Env {
    dir: tempfile::TempDir,
}

impl Env {
    fn new(bbr_available: bool) -> Env {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fake_sysctl(root);
        let st = root.join("state");
        fs::create_dir_all(&st).unwrap();
        let set = |k: &str, v: &str| fs::write(st.join(k), format!("{v}\n")).unwrap();
        set("net.ipv4.tcp_available_congestion_control", if bbr_available { "reno cubic bbr" } else { "reno cubic" });
        set("net.ipv4.tcp_congestion_control", "cubic");
        set("net.core.default_qdisc", "pfifo_fast");
        set("net.core.rmem_max", "212992");
        set("net.core.wmem_max", "212992");
        set("net.ipv4.tcp_rmem", "4096 131072 6291456");
        set("net.ipv4.tcp_wmem", "4096 16384 4194304");
        set("net.ipv4.tcp_slow_start_after_idle", "1");
        set("net.ipv4.tcp_mtu_probing", "0");
        Env { dir }
    }
    fn root(&self) -> &Path {
        self.dir.path()
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new("sh")
            .arg(helper())
            .args(args)
            .env("NEHIR_NETTUNE_ROOT", self.root())
            .env("NEHIR_SYSCTL", self.root().join("fake-sysctl"))
            .env("NEHIR_MODPROBE", "true")
            .env("NEHIR_NMCLI", "true")
            .output()
            .unwrap()
    }
    fn state(&self, key: &str) -> String {
        fs::read_to_string(self.root().join("state").join(key)).unwrap().trim().to_string()
    }
    fn exists(&self, rel: &str) -> bool {
        self.root().join(rel).exists()
    }
}

#[test]
fn apply_writes_settings_backs_up_originals_and_revert_restores() {
    let e = Env::new(true);
    let out = e.run(&["apply", "tcp"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));

    assert_eq!(e.state("net.ipv4.tcp_congestion_control"), "bbr");
    assert_eq!(e.state("net.core.default_qdisc"), "fq");
    assert_eq!(e.state("net.core.rmem_max"), "33554432");
    assert_eq!(e.state("net.ipv4.tcp_wmem"), "4096 65536 33554432");
    assert_eq!(e.state("net.ipv4.tcp_slow_start_after_idle"), "0");
    assert_eq!(e.state("net.ipv4.tcp_mtu_probing"), "1");
    assert!(e.exists("etc/sysctl.d/99-nehir-tcp.conf"));
    assert!(e.exists("etc/modules-load.d/nehir-bbr.conf"));
    let backup = fs::read_to_string(e.root().join("var/lib/nehir/nettune-backup.conf")).unwrap();
    assert!(backup.contains("net.ipv4.tcp_congestion_control = cubic"));
    assert!(backup.contains("net.core.rmem_max = 212992"));

    // İkinci uygulama, özgün yedeği bozmamalı.
    assert!(e.run(&["apply", "tcp"]).status.success());
    let again = fs::read_to_string(e.root().join("var/lib/nehir/nettune-backup.conf")).unwrap();
    assert_eq!(backup, again);

    // Geri alma: her şey özgün değerine döner, dosyalar silinir.
    assert!(e.run(&["revert", "tcp"]).status.success());
    assert_eq!(e.state("net.ipv4.tcp_congestion_control"), "cubic");
    assert_eq!(e.state("net.core.default_qdisc"), "pfifo_fast");
    assert_eq!(e.state("net.core.rmem_max"), "212992");
    assert_eq!(e.state("net.ipv4.tcp_rmem"), "4096 131072 6291456");
    assert_eq!(e.state("net.ipv4.tcp_slow_start_after_idle"), "1");
    assert!(!e.exists("etc/sysctl.d/99-nehir-tcp.conf"));
    assert!(!e.exists("etc/modules-load.d/nehir-bbr.conf"));
    assert!(!e.exists("var/lib/nehir/nettune-backup.conf"));
}

#[test]
fn without_bbr_support_only_safe_buffer_settings_are_written() {
    let e = Env::new(false);
    assert!(e.run(&["apply", "tcp"]).status.success());
    let conf = fs::read_to_string(e.root().join("etc/sysctl.d/99-nehir-tcp.conf")).unwrap();
    assert!(!conf.contains("bbr") && !conf.contains("default_qdisc"));
    assert!(conf.contains("tcp_slow_start_after_idle = 0"));
    assert!(!e.exists("etc/modules-load.d/nehir-bbr.conf"));
    assert_eq!(e.state("net.ipv4.tcp_congestion_control"), "cubic"); // dokunulmadı
}

#[test]
fn wifi_power_save_toggle_is_reversible() {
    let e = Env::new(true);
    assert!(e.run(&["apply", "wifi"]).status.success());
    let conf = fs::read_to_string(e.root().join("etc/NetworkManager/conf.d/99-nehir-wifi.conf")).unwrap();
    assert!(conf.contains("[connection]") && conf.contains("wifi.powersave = 2"));
    assert!(!e.exists("etc/sysctl.d/99-nehir-tcp.conf"), "yalnızca istenen parça uygulanmalı");
    assert!(e.run(&["revert", "wifi"]).status.success());
    assert!(!e.exists("etc/NetworkManager/conf.d/99-nehir-wifi.conf"));
}

#[test]
fn both_parts_in_one_call() {
    let e = Env::new(true);
    assert!(e.run(&["apply", "tcp", "wifi"]).status.success());
    assert!(e.exists("etc/sysctl.d/99-nehir-tcp.conf") && e.exists("etc/NetworkManager/conf.d/99-nehir-wifi.conf"));
    assert!(e.run(&["revert", "tcp", "wifi"]).status.success());
    assert!(!e.exists("etc/sysctl.d/99-nehir-tcp.conf") && !e.exists("etc/NetworkManager/conf.d/99-nehir-wifi.conf"));
}

#[test]
fn rejects_bad_usage_without_touching_anything() {
    let e = Env::new(true);
    for args in [vec!["apply"], vec!["format", "tcp"], vec!["apply", "tcp", "../../etc/passwd"], vec!["apply", "rm -rf /"], vec![]] {
        let out = e.run(&args);
        assert_eq!(out.status.code(), Some(64), "{args:?}");
    }
    assert!(!e.exists("etc") && !e.exists("var"));
    assert_eq!(e.state("net.ipv4.tcp_congestion_control"), "cubic");
}

#[test]
fn refuses_to_run_unprivileged_on_the_real_system() {
    // Sahte kök verilmezse ve root değilsek hiçbir şey yapmadan 77 ile çıkmalı.
    if std::env::var("USER").map(|u| u == "root").unwrap_or(false) {
        return; // root olarak çalışan ortamda bu güvenlik denetimi anlamlı değil
    }
    let out = Command::new("sh").arg(helper()).args(["apply", "tcp"]).env_remove("NEHIR_NETTUNE_ROOT").output().unwrap();
    assert_eq!(out.status.code(), Some(77));
}
