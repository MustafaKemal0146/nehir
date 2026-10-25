//! Tarayıcılar için native messaging köprüsü.
//!
//! Eklenti `runtime.sendNativeMessage("org.nehir.host", {action:"launchApp"})` çağırdığında tarayıcı
//! bu programı başlatır; program Nehir'i arka planda açar. URL, çerez gibi hiçbir veri buradan geçmez.
//!
//! Protokol: stdin/stdout üzerinde 32 bit (yerel bayt sırası) uzunluk + UTF-8 JSON.

use serde_json::{json, Value};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn read_message(input: &mut impl Read) -> Option<Value> {
    let mut len = [0u8; 4];
    input.read_exact(&mut len).ok()?;
    let len = u32::from_ne_bytes(len) as usize;
    if len == 0 || len > 1 << 20 {
        return None;
    }
    let mut buf = vec![0u8; len];
    input.read_exact(&mut buf).ok()?;
    serde_json::from_slice(&buf).ok()
}

fn write_message(out: &mut impl Write, v: &Value) {
    let body = serde_json::to_vec(v).unwrap_or_default();
    let _ = out.write_all(&(body.len() as u32).to_ne_bytes());
    let _ = out.write_all(&body);
    let _ = out.flush();
}

fn app_candidates() -> Vec<PathBuf> {
    let mut c = Vec::new();
    if let Some(p) = std::env::var_os("NEHIR_APP") {
        c.push(PathBuf::from(p));
    }
    if let Ok(exe) = std::env::current_exe().and_then(|p| p.canonicalize()) {
        if let Some(dir) = exe.parent() {
            c.push(dir.join(if cfg!(windows) { "nehir.exe" } else { "nehir" }));
        }
    }
    #[cfg(unix)]
    {
        c.push("/usr/bin/nehir".into());
        c.push("/opt/nehir/nehir".into());
    }
    #[cfg(windows)]
    {
        for var in ["LOCALAPPDATA", "ProgramFiles"] {
            if let Some(base) = std::env::var_os(var) {
                let base = PathBuf::from(base);
                // Kullanıcı başına kurulum (NSIS) ya da makine geneli kurulum.
                c.push(base.join("Nehir").join("nehir.exe"));
                c.push(base.join("Programs").join("Nehir").join("nehir.exe"));
            }
        }
    }
    c
}

#[cfg(unix)]
fn is_executable(p: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

#[cfg(windows)]
fn is_executable(p: &std::path::Path) -> bool {
    p.is_file()
}

/// Tarayıcı kapansa bile uygulama yaşasın: Unix'te ayrı süreç grubu, Windows'ta ayrık ve konsolsuz süreç.
fn detach(cmd: &mut Command) -> &mut Command {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0)
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW)
    }
}

fn launch_app() -> bool {
    for exe in app_candidates() {
        if !is_executable(&exe) {
            continue;
        }
        let mut cmd = Command::new(&exe);
        cmd.arg("--background").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        if detach(&mut cmd).spawn().is_ok() {
            return true;
        }
    }
    false
}

fn main() {
    let mut stdin = std::io::stdin().lock();
    let mut stdout = std::io::stdout().lock();
    while let Some(msg) = read_message(&mut stdin) {
        let reply = match msg.get("action").and_then(Value::as_str) {
            Some("launchApp") => json!({"ok": launch_app()}),
            Some("ping") => json!({"ok": true}),
            _ => json!({"ok": false, "error": "unknown action"}),
        };
        write_message(&mut stdout, &reply);
    }
}
