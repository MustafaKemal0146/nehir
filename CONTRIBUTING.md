# Katkıda bulunma

Katkıya açığız: hata bildirimi, öneri, belge düzeltmesi ya da kod.

## Geliştirme ortamı

```bash
sudo apt install -y libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev \
    librsvg2-dev libxdo-dev build-essential pkg-config curl
git clone https://github.com/MustafaKemal0146/nehir && cd nehir
cargo test
(cd ui && npm install && npm run build)
node extension/build.mjs
cargo run -p nehir-app
```

Mimari için [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)'ye bak.

## Pull request öncesi

- `cargo test` ve `cargo clippy --all-targets` temiz olsun.
- Davranış değişikliğine test ekle (`crates/nehir-core/tests/`).
- Arayüz metinleri için hem Türkçe hem İngilizce sözlüğü güncelle (`ui/src/i18n.js`, `extension/src/i18n.js`).
- Eklenti değiştiyse `node extension/build.mjs` ile derle ve Chromium'da dene.
- Commit mesajları kısa ve açıklayıcı olsun.

## Notlar

- Eklenti sürümü `Cargo.toml`'daki sürümden otomatik gelir; ayrı güncelleme gerekmez.
- `extension/chrome-key.pem` (eklentinin özel anahtarı) depoya **konmaz**; yalnızca genel anahtar (`chrome-key.pub.b64`) vardır ve sabit eklenti kimliğini sağlar.
