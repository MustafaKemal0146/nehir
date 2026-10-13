//! Tarayıcılar için native messaging köprüsü.
//!
//! Eklenti `runtime.sendNativeMessage("org.nehir.host", {action:"launchApp"})` çağırdığında tarayıcı
//! bu programı başlatır; program Nehir'i arka planda açar. URL, çerez gibi hiçbir veri buradan geçmez.
//!
//! Protokol: stdin/stdout üzerinde 32 bit (yerel bayt sırası) uzunluk + UTF-8 JSON.

use serde_json::{json, Value};
use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
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
            c.push(dir.join("nehir"));
        }
    }
    c.push("/usr/bin/nehir".into());
    c.push("/opt/nehir/nehir".into());
    c
}

fn launch_app() -> bool {
    use std::os::unix::fs::PermissionsExt;
    for exe in app_candidates() {
        let ok = std::fs::metadata(&exe).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false);
        if !ok {
            continue;
        }
        // Tarayıcı kapansa bile uygulama yaşasın: ayrı süreç grubu.
        if Command::new(&exe)
            .arg("--background")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .is_ok()
        {
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
