// Eklentiyi dist/chrome ve dist/firefox klasörlerine derler (+ isteğe bağlı zip).
// Kullanım: node build.mjs [--zip]
import { cpSync, mkdirSync, readFileSync, rmSync, writeFileSync, existsSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = dirname(fileURLToPath(import.meta.url));
const src = join(root, 'src');
const dist = join(root, 'dist');
const base = JSON.parse(readFileSync(join(src, 'manifest.base.json'), 'utf8'));
const FIREFOX_ID = 'nehir@nehir.app';
// Eklenti sürümü her zaman uygulama sürümüyle aynıdır (kökteki Cargo.toml [workspace.package]).
const cargo = readFileSync(join(root, '..', 'Cargo.toml'), 'utf8');
const VERSION = /^version\s*=\s*"([^"]+)"/m.exec(cargo)?.[1];
if (!VERSION) throw new Error('Cargo.toml içinde sürüm bulunamadı');

// Sabit Chrome eklenti kimliği için genel anahtar (varsa). Kimlik, native host manifestlerinde kullanılır.
const keyFile = join(root, 'chrome-key.pub.b64');
const chromeKey = existsSync(keyFile) ? readFileSync(keyFile, 'utf8').trim() : null;

rmSync(dist, { recursive: true, force: true });
for (const target of ['chrome', 'firefox']) {
  const out = join(dist, target);
  mkdirSync(out, { recursive: true });
  for (const f of ['i18n.js', 'background.js', 'content.js', 'popup.html', 'popup.js', 'popup.css', 'icons', '_locales']) {
    cpSync(join(src, f), join(out, f), { recursive: true });
  }
  const m = structuredClone(base);
  m.version = VERSION;
  if (target === 'chrome') {
    m.background = { service_worker: 'background.js' };
    if (chromeKey) m.key = chromeKey;
    m.minimum_chrome_version = '110';
  } else {
    m.background = { scripts: ['i18n.js', 'background.js'] };
    m.browser_specific_settings = { gecko: { id: FIREFOX_ID, strict_min_version: '121.0', data_collection_permissions: { required: ['browsingActivity', 'websiteContent'] } } };
  }
  writeFileSync(join(out, 'manifest.json'), JSON.stringify(m, null, 2));
  if (process.argv.includes('--zip')) {
    execFileSync('zip', ['-qr', join(dist, `nehir-${target}-${VERSION}.zip`), '.'], { cwd: out });
  }
}
console.log(`derlendi (sürüm ${VERSION}) →`, dist);
