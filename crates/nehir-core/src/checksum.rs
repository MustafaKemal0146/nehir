//! Dosya bütünlüğü: indirilen dosyanın özetini (MD5, SHA-1, SHA-256, SHA-512) hesaplar ve beklenenle karşılaştırır.
//!
//! Beklenen değer `"sha256:ab12…"` biçiminde ya da yalnızca onaltılık metin olarak verilebilir; algoritma
//! verilmezse uzunluktan anlaşılır (32 → MD5, 40 → SHA-1, 64 → SHA-256, 128 → SHA-512).

use sha2::Digest;
use std::io::Read;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Algo {
    Md5,
    Sha1,
    Sha256,
    Sha512,
}

impl Algo {
    pub fn name(self) -> &'static str {
        match self {
            Algo::Md5 => "md5",
            Algo::Sha1 => "sha1",
            Algo::Sha256 => "sha256",
            Algo::Sha512 => "sha512",
        }
    }
    fn from_name(s: &str) -> Option<Algo> {
        match s.to_ascii_lowercase().replace(['-', '_'], "").as_str() {
            "md5" => Some(Algo::Md5),
            "sha1" => Some(Algo::Sha1),
            "sha256" => Some(Algo::Sha256),
            "sha512" => Some(Algo::Sha512),
            _ => None,
        }
    }
    fn from_len(n: usize) -> Option<Algo> {
        match n {
            32 => Some(Algo::Md5),
            40 => Some(Algo::Sha1),
            64 => Some(Algo::Sha256),
            128 => Some(Algo::Sha512),
            _ => None,
        }
    }
}

/// Beklenen özeti doğrular ve `"algo:hex"` (küçük harf) biçimine getirir. Geçersizse `None`.
pub fn normalize(input: &str) -> Option<String> {
    let s = input.trim();
    let (algo, hex) = match s.split_once(':') {
        Some((a, h)) => (Some(Algo::from_name(a.trim())?), h.trim()),
        None => (None, s),
    };
    if hex.is_empty() || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let by_len = Algo::from_len(hex.len())?;
    if algo.is_some_and(|a| a != by_len) {
        return None;
    }
    Some(format!("{}:{}", by_len.name(), hex.to_ascii_lowercase()))
}

pub fn algo_of(normalized: &str) -> Option<Algo> {
    normalized.split_once(':').and_then(|(a, _)| Algo::from_name(a))
}

/// Dosyanın özetini hesaplar: `"algo:hex"`. Büyük dosyalarda saniyeler sürebilir; `spawn_blocking` içinde çağırın.
pub fn hash_file(path: &Path, algo: Algo) -> std::io::Result<String> {
    fn run<D: Digest>(mut f: std::fs::File) -> std::io::Result<String> {
        let mut h = D::new();
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = f.read(&mut buf)?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
        }
        Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
    }
    let f = std::fs::File::open(path)?;
    let hex = match algo {
        Algo::Md5 => run::<md5::Md5>(f)?,
        Algo::Sha1 => run::<sha1::Sha1>(f)?,
        Algo::Sha256 => run::<sha2::Sha256>(f)?,
        Algo::Sha512 => run::<sha2::Sha512>(f)?,
    };
    Ok(format!("{}:{hex}", algo.name()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_by_length_and_prefix() {
        let h = "E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855";
        assert_eq!(normalize(h).unwrap(), format!("sha256:{}", h.to_lowercase()));
        assert_eq!(normalize(&format!("SHA-256: {h}")).unwrap(), format!("sha256:{}", h.to_lowercase()));
        assert_eq!(normalize("d41d8cd98f00b204e9800998ecf8427e").unwrap(), "md5:d41d8cd98f00b204e9800998ecf8427e");
        assert!(normalize("md5:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855").is_none());
        assert!(normalize("xyz").is_none());
        assert!(normalize("").is_none());
    }

    #[test]
    fn hashes_known_values() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.txt");
        std::fs::write(&p, b"abc").unwrap();
        assert_eq!(hash_file(&p, Algo::Md5).unwrap(), "md5:900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(hash_file(&p, Algo::Sha1).unwrap(), "sha1:a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(hash_file(&p, Algo::Sha256).unwrap(), "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
}
