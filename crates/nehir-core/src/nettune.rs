//! Ağ (TCP/IP ve sürücü) durumunu inceler ve iyileştirme önerir.
//!
//! Burada **hiçbir sistem ayarı değiştirilmez**: yalnızca `/proc` ve `/sys` salt okunur okunur.
//! Değişiklik, kullanıcının onayıyla ve yönetici yetkisiyle `nehir-nettune` yardımcısı tarafından yapılır
//! (bkz. `packaging/nehir-nettune`); her şey geri alınabilir.

use serde::Serialize;
use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;

/// Yardımcı betiğin kurulu yolu (`.deb` ile gelir).
pub const HELPER_PATH: &str = "/usr/lib/nehir/nehir-nettune";

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SysctlItem {
    /// `net.ipv4.tcp_congestion_control` gibi.
    pub key: String,
    /// Arayüzde gösterilecek kısa ad (kod; arayüz yerelleştirir).
    pub id: String,
    pub current: Option<String>,
    pub recommended: String,
    /// Zaten yeterince iyi mi?
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Iface {
    pub name: String,
    /// "wifi" | "ethernet" | "vpn"
    pub kind: String,
    pub driver: Option<String>,
    pub up: bool,
    pub speed_mbps: Option<i64>,
    pub mtu: u32,
    /// İnternete çıkış (varsayılan rota) bu arayüzden mi?
    pub default_route: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetReport {
    /// Bu işletim sisteminde ağ optimizasyonu var mı? Windows'ta `false`: arayüz bölümü gizler.
    pub supported: bool,
    pub items: Vec<SysctlItem>,
    pub ifaces: Vec<Iface>,
    pub bbr_available: bool,
    /// Varsayılan rota bir VPN/tünel üzerinden geçiyor: hız düşer.
    pub vpn_in_route: bool,
    pub wifi_present: bool,
    /// Önerilen TCP ayarlarının tümü zaten uygun.
    pub tcp_ok: bool,
    /// Nehir'in yazdığı ayar dosyaları var mı (geri alınabilir)?
    pub tcp_applied: bool,
    pub wifi_applied: bool,
    pub helper_installed: bool,
}

fn read(root: &Path, rel: &str) -> Option<String> {
    std::fs::read_to_string(root.join(rel.trim_start_matches('/'))).ok().map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
}

fn sysctl_path(key: &str) -> String {
    format!("proc/sys/{}", key.replace('.', "/"))
}

fn max_of(v: &str) -> Option<u64> {
    v.split_whitespace().last().and_then(|s| s.parse().ok())
}

const BUF: u64 = 16 * 1024 * 1024; // 16 MB: hızlı/yüksek gecikmeli hatlar için yeterli tavan
pub const REC_RMEM: &str = "4096 131072 33554432";
pub const REC_WMEM: &str = "4096 65536 33554432";

/// Önerilen değerler ve "zaten yeterli mi" kuralı. `bbr`: çekirdek BBR sunuyor mu.
pub fn evaluate(get: impl Fn(&str) -> Option<String>, bbr: bool) -> Vec<SysctlItem> {
    let mut items = Vec::new();
    let mut add = |id: &str, key: &str, recommended: &str, ok: &dyn Fn(Option<&str>) -> bool| {
        let current = get(key);
        let good = ok(current.as_deref());
        items.push(SysctlItem { key: key.into(), id: id.into(), current, recommended: recommended.into(), ok: good });
    };
    if bbr {
        add("congestion", "net.ipv4.tcp_congestion_control", "bbr", &|c| c == Some("bbr"));
    }
    add("qdisc", "net.core.default_qdisc", "fq", &|c| matches!(c, Some("fq" | "fq_codel" | "cake")));
    add("rmem_max", "net.core.rmem_max", "33554432", &|c| c.and_then(|v| v.parse::<u64>().ok()).map(|v| v >= BUF).unwrap_or(false));
    add("wmem_max", "net.core.wmem_max", "33554432", &|c| c.and_then(|v| v.parse::<u64>().ok()).map(|v| v >= BUF).unwrap_or(false));
    add("tcp_rmem", "net.ipv4.tcp_rmem", REC_RMEM, &|c| c.and_then(max_of).map(|v| v >= BUF).unwrap_or(false));
    add("tcp_wmem", "net.ipv4.tcp_wmem", REC_WMEM, &|c| c.and_then(max_of).map(|v| v >= BUF).unwrap_or(false));
    add("slow_start_idle", "net.ipv4.tcp_slow_start_after_idle", "0", &|c| c == Some("0"));
    add("mtu_probing", "net.ipv4.tcp_mtu_probing", "1", &|c| matches!(c, Some("1" | "2")));
    items
}

fn is_vpn_name(n: &str) -> bool {
    let n = n.to_ascii_lowercase();
    ["tun", "tap", "wg", "ppp", "tailscale", "warp", "cloudflarewarp", "zt", "nordlynx", "proton", "vpn", "ipsec"].iter().any(|p| n.starts_with(p))
}

fn default_route_dev(root: &Path) -> Option<String> {
    let t = std::fs::read_to_string(root.join("proc/net/route")).ok()?;
    t.lines().skip(1).find_map(|l| {
        let f: Vec<&str> = l.split_whitespace().collect();
        (f.len() > 7 && f[1] == "00000000" && f[7] == "00000000").then(|| f[0].to_string())
    })
}

/// Fiziksel ve tünel arayüzleri (köprü/sanal `veth`/`docker` hariç).
pub fn interfaces(root: &Path) -> Vec<Iface> {
    let route = default_route_dev(root);
    let Ok(rd) = std::fs::read_dir(root.join("sys/class/net")) else { return Vec::new() };
    let mut out = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name == "lo" || name.starts_with("veth") || name.starts_with("br-") || name.starts_with("docker") || name.starts_with("virbr") {
            continue;
        }
        let dir = e.path();
        let wifi = dir.join("wireless").exists() || dir.join("phy80211").exists();
        let has_device = dir.join("device").exists();
        let vpn = !has_device && is_vpn_name(&name);
        if !has_device && !vpn {
            continue;
        }
        let driver = std::fs::read_link(dir.join("device/driver")).ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));
        let state = std::fs::read_to_string(dir.join("operstate")).unwrap_or_default();
        let speed = std::fs::read_to_string(dir.join("speed")).ok().and_then(|s| s.trim().parse::<i64>().ok()).filter(|v| *v > 0);
        out.push(Iface {
            kind: if vpn { "vpn" } else if wifi { "wifi" } else { "ethernet" }.into(),
            driver,
            up: state.trim() == "up" || (vpn && state.trim() == "unknown"),
            speed_mbps: speed,
            mtu: std::fs::read_to_string(dir.join("mtu")).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0),
            default_route: route.as_deref() == Some(name.as_str()),
            name,
        });
    }
    out.sort_by(|a, b| b.default_route.cmp(&a.default_route).then(b.up.cmp(&a.up)).then(a.name.cmp(&b.name)));
    out
}

/// `root` = "/" (gerçek sistem) ya da testler için sahte bir kök.
pub fn inspect_at(root: &Path, helper: &Path) -> NetReport {
    let available = read(root, &sysctl_path("net.ipv4.tcp_available_congestion_control")).unwrap_or_default();
    let bbr = available.split_whitespace().any(|c| c == "bbr")
        || root.join("lib/modules").exists() && std::fs::read_dir(root.join("lib/modules")).map(|d| d.flatten().any(|e| e.path().join("kernel/net/ipv4/tcp_bbr.ko.zst").exists() || e.path().join("kernel/net/ipv4/tcp_bbr.ko").exists())).unwrap_or(false);
    let items = evaluate(|k| read(root, &sysctl_path(k)), bbr);
    let ifaces = interfaces(root);
    let tcp_ok = items.iter().all(|i| i.ok);
    NetReport {
        supported: true,
        bbr_available: bbr,
        vpn_in_route: ifaces.iter().any(|i| i.kind == "vpn" && i.default_route),
        wifi_present: ifaces.iter().any(|i| i.kind == "wifi"),
        tcp_ok,
        tcp_applied: root.join("etc/sysctl.d/99-nehir-tcp.conf").exists(),
        wifi_applied: root.join("etc/NetworkManager/conf.d/99-nehir-wifi.conf").exists(),
        helper_installed: helper.exists(),
        items,
        ifaces,
    }
}

#[cfg(unix)]
pub fn inspect() -> NetReport {
    inspect_at(Path::new("/"), &PathBuf::from(HELPER_PATH))
}

/// Windows'ta `/proc/sys` yoktur ve TCP ayarlarını (BBR, tamponlar) elle değiştirmek Windows'un
/// kendi otomatik ayarını bozabilir; bu yüzden özellik yoktur.
#[cfg(not(unix))]
pub fn inspect() -> NetReport {
    NetReport {
        supported: false,
        items: Vec::new(),
        ifaces: Vec::new(),
        bbr_available: false,
        vpn_in_route: false,
        wifi_present: false,
        tcp_ok: true,
        tcp_applied: false,
        wifi_applied: false,
        helper_installed: false,
    }
}

#[cfg(not(unix))]
pub fn run_helper(_op: &str, _parts: &[&str]) -> Result<(), String> {
    Err("unsupported".into())
}

/// Yardımcıyı `pkexec` ile çalıştırır (yönetici parolası ister). `op`: "apply" | "revert"; `parts`: "tcp", "wifi".
#[cfg(unix)]
pub fn run_helper(op: &str, parts: &[&str]) -> Result<(), String> {
    if !matches!(op, "apply" | "revert") || parts.is_empty() || parts.iter().any(|p| !matches!(*p, "tcp" | "wifi")) {
        return Err("geçersiz istek".into());
    }
    if !Path::new(HELPER_PATH).exists() {
        return Err("helper_missing".into());
    }
    let out = std::process::Command::new("pkexec")
        .arg(HELPER_PATH)
        .arg(op)
        .args(parts)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("pkexec başlatılamadı: {e}"))?;
    match out.status.code() {
        Some(0) => Ok(()),
        Some(126) | Some(127) => Err("cancelled".into()), // parola penceresi iptal / yetki verilmedi
        _ => Err(String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or("yardımcı hata verdi").to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(root: &Path, rel: &str, v: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, v).unwrap();
    }

    fn fake_system(root: &Path, bbr: &str, slow_idle: &str) {
        write(root, "proc/sys/net/ipv4/tcp_available_congestion_control", "reno cubic bbr\n");
        write(root, "proc/sys/net/ipv4/tcp_congestion_control", &format!("{bbr}\n"));
        write(root, "proc/sys/net/core/default_qdisc", "fq\n");
        write(root, "proc/sys/net/core/rmem_max", "212992\n");
        write(root, "proc/sys/net/core/wmem_max", "212992\n");
        write(root, "proc/sys/net/ipv4/tcp_rmem", "4096\t131072\t6291456\n");
        write(root, "proc/sys/net/ipv4/tcp_wmem", "4096\t16384\t4194304\n");
        write(root, "proc/sys/net/ipv4/tcp_slow_start_after_idle", &format!("{slow_idle}\n"));
        write(root, "proc/sys/net/ipv4/tcp_mtu_probing", "0\n");
    }

    #[test]
    fn flags_what_is_not_good_enough() {
        let d = tempfile::tempdir().unwrap();
        fake_system(d.path(), "cubic", "1");
        let r = inspect_at(d.path(), Path::new("/nonexistent"));
        assert!(r.bbr_available && !r.tcp_ok && !r.helper_installed);
        let by = |id: &str| r.items.iter().find(|i| i.id == id).unwrap().clone();
        assert!(!by("congestion").ok && by("congestion").current.as_deref() == Some("cubic"));
        assert!(by("qdisc").ok);
        assert!(!by("rmem_max").ok && !by("tcp_rmem").ok && !by("tcp_wmem").ok);
        assert!(!by("slow_start_idle").ok && !by("mtu_probing").ok);
    }

    #[test]
    fn already_tuned_system_is_ok() {
        let d = tempfile::tempdir().unwrap();
        fake_system(d.path(), "bbr", "0");
        write(d.path(), "proc/sys/net/core/rmem_max", "33554432\n");
        write(d.path(), "proc/sys/net/core/wmem_max", "33554432\n");
        write(d.path(), "proc/sys/net/ipv4/tcp_rmem", "4096 131072 33554432\n");
        write(d.path(), "proc/sys/net/ipv4/tcp_wmem", "4096 65536 33554432\n");
        write(d.path(), "proc/sys/net/ipv4/tcp_mtu_probing", "1\n");
        let r = inspect_at(d.path(), Path::new("/nonexistent"));
        assert!(r.tcp_ok, "{:?}", r.items.iter().filter(|i| !i.ok).collect::<Vec<_>>());
    }

    #[test]
    fn omits_bbr_row_when_kernel_lacks_it() {
        let d = tempfile::tempdir().unwrap();
        fake_system(d.path(), "cubic", "1");
        write(d.path(), "proc/sys/net/ipv4/tcp_available_congestion_control", "reno cubic\n");
        let r = inspect_at(d.path(), Path::new("/nonexistent"));
        assert!(!r.bbr_available);
        assert!(r.items.iter().all(|i| i.id != "congestion"));
    }

    #[test]
    fn detects_interfaces_and_vpn_route() {
        let d = tempfile::tempdir().unwrap();
        fake_system(d.path(), "bbr", "0");
        // Wi-Fi (iwlwifi), Ethernet (kapalı), VPN (varsayılan rota), docker köprüsü (yok sayılır)
        write(d.path(), "sys/class/net/wlo1/operstate", "up\n");
        write(d.path(), "sys/class/net/wlo1/mtu", "1500\n");
        fs::create_dir_all(d.path().join("sys/class/net/wlo1/wireless")).unwrap();
        fs::create_dir_all(d.path().join("sys/class/net/wlo1/device")).unwrap();
        write(d.path(), "sys/class/net/enp3s0/operstate", "down\n");
        write(d.path(), "sys/class/net/enp3s0/mtu", "1500\n");
        fs::create_dir_all(d.path().join("sys/class/net/enp3s0/device")).unwrap();
        write(d.path(), "sys/class/net/CloudflareWARP/operstate", "unknown\n");
        write(d.path(), "sys/class/net/CloudflareWARP/mtu", "1280\n");
        write(d.path(), "sys/class/net/docker0/operstate", "up\n");
        write(d.path(), "proc/net/route", "Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\nCloudflareWARP\t00000000\t0100A8C0\t0003\t0\t0\t0\t00000000\n");
        let r = inspect_at(d.path(), Path::new("/nonexistent"));
        let names: Vec<_> = r.ifaces.iter().map(|i| (i.name.as_str(), i.kind.as_str())).collect();
        assert_eq!(names, vec![("CloudflareWARP", "vpn"), ("wlo1", "wifi"), ("enp3s0", "ethernet")]);
        assert!(r.vpn_in_route && r.wifi_present);
        assert_eq!(r.ifaces[0].mtu, 1280);
    }

    #[test]
    fn helper_rejects_unknown_requests() {
        assert!(run_helper("format", &["tcp"]).is_err());
        assert!(run_helper("apply", &[]).is_err());
        assert!(run_helper("apply", &["rm -rf /"]).is_err());
    }
}
