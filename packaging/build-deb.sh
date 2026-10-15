#!/usr/bin/env bash
# Nehir .deb paketi üretir. Önce: (cd ui && npm run build) && node extension/build.mjs && cargo build --release -p nehir-app --features custom-protocol && cargo build --release -p nehir-host
# Kullanım: packaging/build-deb.sh [sürüm]
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VERSION="${1:-$(grep -m1 '^version' "$ROOT/Cargo.toml" | sed 's/.*"\(.*\)"/\1/')}"
ARCH="$(dpkg --print-architecture)"
REL="$ROOT/target/release"
CHROME_ID="$(tr -d '[:space:]' < "$ROOT/extension/chrome-extension-id.txt")"
FIREFOX_ID="nehir@nehir.app"
OUT="$ROOT/dist"
PKG="$OUT/pkg"

for f in "$REL/nehir" "$REL/nehir-native-host" "$ROOT/extension/dist/chrome/manifest.json"; do
  [ -e "$f" ] || { echo "eksik: $f (önce derleyin)" >&2; exit 1; }
done

rm -rf "$PKG"; mkdir -p "$OUT"
install -Dm755 "$REL/nehir"              "$PKG/usr/bin/nehir"
install -Dm755 "$REL/nehir-native-host"  "$PKG/usr/lib/nehir/nehir-native-host"
install -Dm755 "$ROOT/packaging/nehir-nettune" "$PKG/usr/lib/nehir/nehir-nettune"
install -Dm644 "$ROOT/packaging/org.nehir.nettune.policy" "$PKG/usr/share/polkit-1/actions/org.nehir.nettune.policy"
install -Dm644 "$ROOT/packaging/org.nehir.app.desktop" "$PKG/usr/share/applications/org.nehir.app.desktop"
install -Dm644 "$ROOT/LICENSE" "$PKG/usr/share/doc/nehir/copyright"
for s in 16 32 48 96 128 256 512; do
  install -Dm644 "$ROOT/assets/nehir-$s.png" "$PKG/usr/share/icons/hicolor/${s}x${s}/apps/nehir.png"
done
mkdir -p "$PKG/usr/share/nehir/extension"
cp -r "$ROOT/extension/dist/chrome"  "$PKG/usr/share/nehir/extension/chrome"
cp -r "$ROOT/extension/dist/firefox" "$PKG/usr/share/nehir/extension/firefox"

# Sistem geneli native messaging manifestleri (kullanıcı dizinine ayrıca uygulama kendisi de yazar).
HOST=/usr/lib/nehir/nehir-native-host
chromium_manifest() { cat <<JSON
{
  "name": "org.nehir.host",
  "description": "Nehir Download Manager",
  "path": "$HOST",
  "type": "stdio",
  "allowed_origins": ["chrome-extension://$CHROME_ID/"]
}
JSON
}
firefox_manifest() { cat <<JSON
{
  "name": "org.nehir.host",
  "description": "Nehir Download Manager",
  "path": "$HOST",
  "type": "stdio",
  "allowed_extensions": ["$FIREFOX_ID"]
}
JSON
}
for dir in etc/opt/chrome etc/chromium etc/opt/edge; do
  mkdir -p "$PKG/$dir/native-messaging-hosts"
  chromium_manifest > "$PKG/$dir/native-messaging-hosts/org.nehir.host.json"
done
mkdir -p "$PKG/usr/lib/mozilla/native-messaging-hosts"
firefox_manifest > "$PKG/usr/lib/mozilla/native-messaging-hosts/org.nehir.host.json"

find "$PKG" -type d -exec chmod 755 {} +
find "$PKG/etc" "$PKG/usr/lib/mozilla" -type f -exec chmod 644 {} +
SIZE=$(du -sk "$PKG" | cut -f1)
mkdir -p "$PKG/DEBIAN"
cat > "$PKG/DEBIAN/control" <<CTL
Package: nehir
Version: $VERSION
Section: net
Priority: optional
Architecture: $ARCH
Installed-Size: $SIZE
Maintainer: Nehir <nehir@users.noreply.github.com>
Depends: libwebkit2gtk-4.1-0, libgtk-3-0, libayatana-appindicator3-1 | libappindicator3-1, xdg-utils
Recommends: ffmpeg, pkexec | policykit-1
Suggests: yt-dlp
Homepage: https://github.com/MustafaKemal0146/nehir
Description: Nehir — fast multi-connection download manager
 IDM-style download manager for Linux. Captures downloads and videos from
 Chrome, Brave, Edge and Firefox through a browser extension, downloads files
 with up to 16 parallel connections (pause/resume supported) and fetches
 videos from 1000+ sites via yt-dlp.
CTL
cat > "$PKG/DEBIAN/postinst" <<'POST'
#!/bin/sh
set -e
command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q -t -f /usr/share/icons/hicolor || true
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q /usr/share/applications || true
exit 0
POST
chmod 755 "$PKG/DEBIAN/postinst"
# Paket kaldırılırken Nehir'in yazdığı ağ ayarları (varsa) özgün değerlerine döndürülür.
cat > "$PKG/DEBIAN/prerm" <<'PRERM'
#!/bin/sh
set -e
case "$1" in
  remove|purge)
    [ -x /usr/lib/nehir/nehir-nettune ] && /usr/lib/nehir/nehir-nettune revert tcp wifi >/dev/null 2>&1 || true
    ;;
esac
exit 0
PRERM
chmod 755 "$PKG/DEBIAN/prerm"

DEB="$OUT/nehir_${VERSION}_${ARCH}.deb"
dpkg-deb --build --root-owner-group "$PKG" "$DEB" >/dev/null
echo "paket: $DEB ($(du -h "$DEB" | cut -f1))"
