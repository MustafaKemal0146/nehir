// Nehir köprü eklentisi — arka plan betiği (Chrome MV3 service worker / Firefox event page)
'use strict';

if (typeof importScripts === 'function') importScripts('i18n.js'); // Chrome service worker (Firefox'ta manifest yükler)
const api = globalThis.browser ?? globalThis.chrome;
const t = (k) => NehirI18n.t(k);

const IS_FIREFOX = String(api.runtime.getURL('')).startsWith('moz-extension:');
const UA = navigator.userAgent;
const BROWSER = IS_FIREFOX ? 'firefox'
  : navigator.brave ? 'brave'
  : /Edg\//.test(UA) ? 'edge'
  : /OPR\//.test(UA) ? 'opera'
  : /Vivaldi/.test(UA) ? 'vivaldi'
  : 'chrome';

const NATIVE_HOST = 'org.nehir.host';
const DEFAULTS = { port: 48213, intercept: true, minSizeMB: 0, sniff: true, autoLaunch: true, overlay: true };

const MEDIA_EXT = /\.(mp4|m4v|webm|mkv|mov|avi|flv|mp3|m4a|aac|ogg|opus|wav|flac)(\?|#|$)/i;
const FRAGMENT = /\.(ts|m4s|m4f|cmfv|cmfa|fmp4|init)(\?|#|$)|[/_.-](seg|segment|chunk|frag|fragment)[-_]?\d+/i;
// Bu sitelerin medya adresleri imzalı/parçalı olduğu için sayfa adresi yt-dlp'ye verilir.
const PAGE_ONLY_HOSTS = /(^|\.)googlevideo\.com$|(^|\.)fbcdn\.net$|(^|\.)twimg\.com$/i;

// ---------- Ayarlar ----------
async function getSettings() {
  const s = await api.storage.local.get('settings');
  return { ...DEFAULTS, ...(s.settings || {}) };
}
async function setSettings(patch) {
  const cur = await getSettings();
  await api.storage.local.set({ settings: { ...cur, ...patch } });
}

// ---------- Uygulama ile iletişim ----------
// Özel başlık, isteğin bir eklentiden geldiğinin kanıtıdır: web sayfaları CORS ön isteği olmadan
// ekleyemez, köprü de ön isteği yalnızca eklenti kökenlerine yanıtlar.
const BRIDGE_HEADERS = { 'X-Nehir-Extension': BROWSER, 'X-Nehir-Extension-Id': api.runtime.id };
const base = (s) => `http://127.0.0.1:${s.port}`;

async function bridge(path, init, settings, timeoutMs = 8000) {
  settings = settings || (await getSettings());
  const ctrl = new AbortController();
  const timer = setTimeout(() => ctrl.abort(), timeoutMs);
  try {
    const r = await fetch(base(settings) + path, {
      cache: 'no-store', ...init, signal: ctrl.signal,
      headers: { ...BRIDGE_HEADERS, ...(init?.headers || {}) },
    });
    return await r.json();
  } finally {
    clearTimeout(timer);
  }
}

async function ping(settings) {
  try {
    const app = await bridge('/ping', null, settings, 1200);
    if (!app || app.ok !== true) return null;
    if (app.active > 0) startProgress();
    if (Array.isArray(app.captureExtensions)) api.storage.local.set({ captureExtensions: app.captureExtensions });
    if (app.language) {
      api.storage.local.set({ appLanguage: app.language });
      if (NehirI18n.setLang(app.language)) createMenus(); // uygulamanın dili değiştiyse menüleri yenile
    }
    return app;
  } catch {
    return null;
  }
}

async function launchApp() {
  if (!api.runtime.sendNativeMessage) return false;
  try {
    const r = await api.runtime.sendNativeMessage(NATIVE_HOST, { action: 'launchApp' });
    return !!(r && r.ok);
  } catch {
    return false;
  }
}

// Uygulama kapalıysa başlatır ve ~12 sn boyunca bekler.
let launching = null;
function ensureApp(settings) {
  if (!launching) {
    launching = (async () => {
      const app = await ping(settings);
      if (app) return app;
      if (!(await launchApp())) return null;
      const until = Date.now() + 12000;
      while (Date.now() < until) {
        await new Promise((r) => setTimeout(r, 400));
        const a = await ping(settings);
        if (a) return a;
      }
      return null;
    })().finally(() => { launching = null; });
  }
  return launching;
}

// ---------- İlerleme: araç çubuğu rozeti ----------
const ACTIVE = new Set(['probing', 'downloading', 'processing']);
let progressTimer = null;
let badgeShown = false;

async function getDownloads() {
  try { return (await bridge('/downloads')).items || []; } catch { return []; }
}
async function setBadge(text, color) {
  try { await api.action.setBadgeText({ text }); if (color) await api.action.setBadgeBackgroundColor({ color }); } catch {}
}
async function tickProgress() {
  const list = await getDownloads();
  const active = list.filter((i) => ACTIVE.has(i.state));
  if (active.length) {
    const measurable = active.filter((i) => i.total > 0);
    let text = '…';
    if (active.length > 1) text = active.length + '↓';
    else if (measurable.length) text = Math.round(100 * measurable[0].progress) + '%';
    badgeShown = true;
    await setBadge(text, '#1f6feb');
  } else {
    stopProgress();
    if (badgeShown) {
      badgeShown = false;
      await setBadge('✓', '#2ea043');
      setTimeout(() => setBadge('', '#1f6feb'), 4000);
    }
  }
}
function startProgress() {
  if (progressTimer) return;
  tickProgress();
  progressTimer = setInterval(tickProgress, 1500); // her istek MV3 worker'ını canlı tutar
}
function stopProgress() {
  if (progressTimer) { clearInterval(progressTimer); progressTimer = null; }
}

function notify(title, message) {
  try {
    api.notifications?.create?.('', {
      type: 'basic', iconUrl: 'icons/icon128.png', title, message: String(message || '').slice(0, 200),
    });
  } catch {}
}

async function cookiesFor(url) {
  try {
    const list = await api.cookies.getAll({ url });
    return {
      header: list.map((c) => `${c.name}=${c.value}`).join('; '),
      list: list.map((c) => ({
        name: c.name, value: c.value, domain: c.domain, path: c.path,
        secure: !!c.secure, expirationDate: c.expirationDate || 0,
      })),
    };
  } catch {
    return { header: '', list: [] };
  }
}

// Sayfa adresi yt-dlp ile indirilemezse uygulamanın deneyeceği, bu sekmede algılanan medya adresleri:
// önce en büyük düz dosyalar, sonra HLS, sonra DASH. İmzalı/parçalı CDN adresleri (pageOnly) hariç.
async function fallbackUrlsFor(tabId, mainUrl) {
  if (tabId == null || tabId < 0) return [];
  const list = (await readMedia(tabId)).filter((m) => !m.pageOnly && m.url !== mainUrl);
  const rank = (m) => (m.kind === 'media' ? 0 : m.kind === 'hls' ? 1 : 2);
  list.sort((a, b) => rank(a) - rank(b) || (b.size || 0) - (a.size || 0));
  return list.slice(0, 6).map((m) => m.url);
}

// Sekme başlığını dosya adına uygun hale getirir: bildirim sayacı "(18) " ve site soneki " - YouTube" atılır.
function cleanTitle(title) {
  return String(title || '').replace(/^\(\d+\)\s*/, '').replace(/\s*[-–|]\s*YouTube\s*$/i, '').trim().slice(0, 150);
}

// ---------- Dosya özeti (bütünlük doğrulaması) ----------
// İndirmenin başladığı sayfada, dosya adının yanında yazan SHA-256 (vb.) ya da sayfadaki SHA256SUMS dosyası.
// Bulunursa uygulama indirme bitince dosyayı doğrular. En çok ~2.5 sn sürer; bulunamazsa indirme yine başlar.
const withTimeout = (p, ms) => Promise.race([p, new Promise((r) => setTimeout(() => r(null), ms))]);

async function findTab(tabId, pageUrl) {
  if (tabId != null && tabId >= 0) return tabId;
  if (!pageUrl) return null;
  try {
    const tabs = await api.tabs.query({});
    const exact = tabs.find((t) => t.url === pageUrl);
    return exact ? exact.id : null;
  } catch { return null; }
}

// Satırda dosya adı tam olarak geçiyor mu? ("server.iso", "live-server.iso" ya da "server.iso.sha256" içinde sayılmaz.)
function hasName(low, names) {
  for (const n of names) {
    for (let i = low.indexOf(n); i >= 0; i = low.indexOf(n, i + 1)) {
      const before = low[i - 1] || '', after = low[i + n.length] || '', next = low[i + n.length + 1] || '';
      if (/[\w.-]/.test(before)) continue;
      if (/[\w-]/.test(after) || (after === '.' && /\w/.test(next))) continue;
      return true;
    }
  }
  return false;
}
function parseSums(text, names) {
  const lines = String(text).split('\n');
  const hexRe = /\b([a-f0-9]{128}|[a-f0-9]{64}|[a-f0-9]{40}|[a-f0-9]{32})\b/i;
  const byLen = { 32: 'md5', 40: 'sha1', 64: 'sha256', 128: 'sha512' };
  const fmt = (h) => `${byLen[h.length]}:${h.toLowerCase()}`;
  for (const line of lines) {
    const low = line.toLowerCase();
    // GNU: "<özet>  *dosya" · BSD: "SHA256 (dosya) = <özet>"
    if (!hasName(low, names)) continue;
    const m = hexRe.exec(line);
    if (m) return fmt(m[1]);
  }
  // Tek dosyalık özet dosyası (dosya.iso.sha256): yalnızca özet yazar.
  const only = lines.map((l) => l.trim()).filter(Boolean);
  if (only.length === 1) { const m = hexRe.exec(only[0]); if (m && m[1].length >= 64) return fmt(m[1]); }
  return null;
}

async function checksumFor(tabId, pageUrl, filename, url) {
  try {
    const id = await findTab(tabId, pageUrl);
    if (id == null) return null;
    const r = await withTimeout(api.tabs.sendMessage(id, { type: 'findChecksum', filename, url }, { frameId: 0 }).catch(() => null), 800);
    if (!r) return null;
    if (r.checksum) return r.checksum;
    const names = new Set([filename, (() => { try { return decodeURIComponent(new URL(url).pathname.split('/').pop()); } catch { return ''; } })()]
      .map((n) => String(n || '').toLowerCase()).filter((n) => n.length >= 3));
    for (const link of (r.links || []).slice(0, 3)) {
      const text = await withTimeout(fetch(link, { credentials: 'omit', cache: 'no-store' })
        .then((res) => (res.ok && Number(res.headers.get('content-length') || 0) < 512 * 1024 ? res.text() : ''))
        .catch(() => ''), 1500);
      const c = text && text.length < 512 * 1024 ? parseSums(text, names) : null;
      if (c) return c;
    }
  } catch {}
  return null;
}

async function sendToApp({ url, filename, referer, kind, tabId, title, audio, quality }) {
  await NehirI18n.ready;
  if (audio) kind = 'video'; // MP3'e çevirme yt-dlp ile yapılır
  const settings = await getSettings();
  const ck = await cookiesFor(url);
  const payload = {
    url, filename: filename || '', referer: referer || '', kind: kind || 'file',
    userAgent: UA, cookieHeader: ck.header, cookies: ck.list,
    fallbackUrls: kind === 'video' ? await fallbackUrlsFor(tabId, url) : [],
    titleHint: cleanTitle(title),
    quality: audio ? 'audio' : quality || undefined,
    checksum: kind === 'file' ? (await checksumFor(tabId, referer, filename, url)) || undefined : undefined,
  };
  const post = () => bridge('/download', {
    method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(payload),
  }, settings);
  try {
    let j;
    try { j = await post(); } catch (e) {
      if (!settings.autoLaunch || !(await ensureApp(settings))) throw e;
      j = await post();
    }
    if (!j.ok) throw new Error(j.error || 'app error');
    notify(audio ? t('notifAudioTitle') : kind === 'video' ? t('notifVideoTitle') : t('notifSentTitle'), filename || title || url);
    startProgress();
    return { ok: true };
  } catch (e) {
    notify(t('notifOfflineTitle'), t('notifOfflineBody'));
    return { ok: false, error: String(e) };
  }
}

// ---------- Yakalanacak dosya türleri (liste uygulamadan gelir) ----------
function extOf(name) {
  const m = String(name || '').split(/[?#]/)[0].match(/\.([a-z0-9]{1,8})$/i);
  return m ? m[1].toLowerCase() : '';
}
function matchesCaptureTypes(list, filename, url) {
  if (!Array.isArray(list) || !list.length) return true;
  let path = '';
  try { path = new URL(url).pathname; } catch {}
  const e = extOf(filename) || extOf(path);
  return !!e && list.includes(e);
}

// ---------- Tarayıcı indirmelerini yakalama ----------
async function interceptDownload(item) {
  const settings = await getSettings();
  if (!settings.intercept) return false;
  const url = item.finalUrl || item.url;
  if (!/^https?:/i.test(url)) return false;          // blob:, data:, file: tarayıcıda kalır
  if (item.byExtensionId) return false;
  if (settings.minSizeMB > 0 && item.totalBytes > 0 && item.totalBytes < settings.minSizeMB * 1048576) return false;

  const { captureExtensions } = await api.storage.local.get('captureExtensions');
  const filename = item.filename ? item.filename.split(/[\\/]/).pop() : '';
  let app = await ping(settings);
  if (!app) {
    // Kapalıysa yalnızca tanıdığımız dosya türleri için uygulamayı başlat.
    if (!settings.autoLaunch || !matchesCaptureTypes(captureExtensions, filename, url)) return false;
    app = await ensureApp(settings);
  }
  if (!app || !matchesCaptureTypes(app.captureExtensions, filename, url)) return false;

  try { await api.downloads.cancel(item.id); } catch {}
  if (IS_FIREFOX) {
    try {
      const [d] = await api.downloads.search({ id: item.id });
      if (d && d.state === 'complete' && d.exists !== false) await api.downloads.removeFile(item.id);
    } catch {}
  }
  try { await api.downloads.erase({ id: item.id }); } catch {}
  sendToApp({ url, filename, referer: item.referrer, kind: 'file' });
  return true;
}

if (api.downloads?.onDeterminingFilename) {
  // Chrome / Edge / Brave: dosya adı belirlenirken indirme henüz başlamadan durdurulabilir.
  api.downloads.onDeterminingFilename.addListener((item, suggest) => {
    interceptDownload(item).then((took) => { if (!took) { try { suggest(); } catch {} } }, () => { try { suggest(); } catch {} });
    return true;
  });
} else if (api.downloads?.onCreated) {
  // Firefox: indirme başlamış olur; uygulama yanıtlayana kadar duraklat, reddederse sürdür.
  api.downloads.onCreated.addListener((item) => {
    const paused = item.state === 'in_progress'
      ? Promise.resolve(api.downloads.pause(item.id)).then(() => true, () => false)
      : Promise.resolve(false);
    interceptDownload(item).then(async (took) => {
      if (!took && (await paused)) { try { await api.downloads.resume(item.id); } catch {} }
    });
  });
}

// ---------- Medya algılama ----------
const mem = new Map(); // tabId → [{...}]  (storage.session yoksa yedek)

async function readMedia(tabId) {
  if (api.storage.session) {
    const key = 'media:' + tabId;
    return (await api.storage.session.get(key))[key] || [];
  }
  return mem.get(tabId) || [];
}
async function writeMedia(tabId, list) {
  if (api.storage.session) await api.storage.session.set({ ['media:' + tabId]: list });
  else mem.set(tabId, list);
  if (!badgeShown) {
    try {
      await api.action.setBadgeText({ tabId, text: list.length ? String(list.length) : '' });
      await api.action.setBadgeBackgroundColor({ tabId, color: '#1f6feb' });
    } catch {}
  }
}
async function clearMedia(tabId) {
  if (api.storage.session) await api.storage.session.remove('media:' + tabId);
  mem.delete(tabId);
  try { await api.action.setBadgeText({ tabId, text: '' }); } catch {}
}

function normalizeKey(url) {
  try {
    const u = new URL(url);
    ['range', 'bytes', 'start', 'end', 'rn', 'rbuf', 'r', '_'].forEach((p) => u.searchParams.delete(p));
    return u.origin + u.pathname + (u.searchParams.toString() ? '?' + u.searchParams.toString() : '');
  } catch { return url; }
}
function guessName(url, type) {
  try {
    const u = new URL(url);
    let n = decodeURIComponent(u.pathname.split('/').filter(Boolean).pop() || '') || u.hostname;
    if (!/\.[a-z0-9]{2,5}$/i.test(n) && type) {
      const ext = { 'video/mp4': 'mp4', 'video/webm': 'webm', 'audio/mpeg': 'mp3', 'audio/mp4': 'm4a', 'video/quicktime': 'mov' }[type.toLowerCase()];
      if (ext) n += '.' + ext;
    }
    return n;
  } catch { return url; }
}

let mediaQueue = Promise.resolve(); // aynı sekmeye eşzamanlı yazımları sıraya sok
function addMedia(tabId, info) {
  if (tabId < 0) return;
  mediaQueue = mediaQueue.then(async () => {
    if (!(await getSettings()).sniff) return;
    const list = await readMedia(tabId);
    const key = normalizeKey(info.url);
    const idx = list.findIndex((m) => m.key === key);
    const entry = { ...info, key, name: info.name || guessName(info.url, info.type) };
    if (idx >= 0) {
      if ((info.size || 0) > (list[idx].size || 0)) { list[idx] = { ...list[idx], ...entry }; await writeMedia(tabId, list); }
      return;
    }
    list.unshift(entry);
    if (list.length > 40) list.length = 40;
    await writeMedia(tabId, list);
  }).catch(() => {});
}

function headerVal(headers, name) {
  const h = (headers || []).find((x) => x.name.toLowerCase() === name);
  return h ? h.value : '';
}
function classify(url, ct) {
  ct = (ct || '').toLowerCase();
  if (/\.m3u8(\?|#|$)/i.test(url) || /mpegurl/.test(ct)) return 'hls';
  if (/\.mpd(\?|#|$)/i.test(url) || /dash\+xml/.test(ct)) return 'dash';
  if (FRAGMENT.test(url)) return null;
  if (/^(video|audio)\//.test(ct)) return /mp2t|iso\.segment/.test(ct) ? null : 'media';
  if (/octet-stream/.test(ct) && MEDIA_EXT.test(url)) return 'media';
  return null;
}

if (api.webRequest?.onHeadersReceived) {
  api.webRequest.onHeadersReceived.addListener(
    (d) => {
      if (d.tabId < 0 || d.statusCode >= 400) return;
      const ct = headerVal(d.responseHeaders, 'content-type');
      const kind = classify(d.url, ct);
      if (!kind) return;
      let size = parseInt(headerVal(d.responseHeaders, 'content-length'), 10) || 0;
      const cr = headerVal(d.responseHeaders, 'content-range').match(/\/(\d+)/);
      if (cr) size = parseInt(cr[1], 10);
      if (kind === 'media' && size > 0 && size < 300 * 1024) return; // küçük parçalar / önizlemeler
      let host = '';
      try { host = new URL(d.url).hostname; } catch {}
      addMedia(d.tabId, { url: d.url, type: ct.split(';')[0], size, kind, time: Date.now(), pageOnly: PAGE_ONLY_HOSTS.test(host) });
    },
    { urls: ['<all_urls>'], types: ['media', 'xmlhttprequest', 'other', 'object', 'main_frame'] },
    ['responseHeaders'],
  );
}

api.tabs.onUpdated.addListener((tabId, info) => { if (info.status === 'loading' && info.url) clearMedia(tabId); });
api.tabs.onRemoved.addListener((tabId) => clearMedia(tabId));

// ---------- Sağ tık menüsü ----------
const menus = api.contextMenus || api.menus;
async function createMenus() {
  if (!menus) return;
  await NehirI18n.ready;
  let created = false;
  const create = () => {
    if (created) return;
    created = true;
    try {
      menus.create({ id: 'nehir-link', title: t('menuLink'), contexts: ['link', 'video', 'audio', 'image'] });
      menus.create({ id: 'nehir-page-video', title: t('menuVideo'), contexts: ['page', 'video', 'frame', 'action'] });
      menus.create({ id: 'nehir-page-audio', title: t('menuAudio'), contexts: ['page', 'video', 'frame', 'action'] });
    } catch {}
  };
  try {
    const r = IS_FIREFOX ? menus.removeAll() : menus.removeAll(create);
    if (r && typeof r.then === 'function') r.then(create, create);
  } catch { create(); }
}
api.runtime.onInstalled.addListener(() => { createMenus(); ping(); });
api.runtime.onStartup?.addListener(() => { createMenus(); ping(); });
menus?.onClicked.addListener((info, tab) => {
  if (info.menuItemId === 'nehir-link') {
    const url = info.linkUrl || info.srcUrl;
    if (url) sendToApp({ url, referer: info.pageUrl || tab?.url, kind: 'file', tabId: tab?.id });
  } else if (info.menuItemId === 'nehir-page-video' || info.menuItemId === 'nehir-page-audio') {
    const url = info.pageUrl || tab?.url;
    if (url) sendToApp({ url, referer: url, kind: 'video', tabId: tab?.id, title: tab?.title, audio: info.menuItemId === 'nehir-page-audio' });
  }
});

// ---------- Popup / içerik betiği iletileri ----------
api.runtime.onMessage.addListener((msg, sender, sendResponse) => {
  (async () => {
    try {
      switch (msg?.type) {
        case 'status': {
          await NehirI18n.ready;
          const settings = await getSettings();
          const app = await ping(settings);
          sendResponse({ online: !!app, app, settings, canLaunch: !!api.runtime.sendNativeMessage, lang: NehirI18n.getLang(), version: api.runtime.getManifest().version });
          break;
        }
        case 'overlayState': {
          const s = await getSettings();
          sendResponse({ enabled: !!s.overlay, lang: NehirI18n.getLang() });
          break;
        }
        case 'formats': {
          // Videonun kullanılabilir çözünürlüklerini uygulamadan (yt-dlp) sorar.
          const settings = await getSettings();
          const ck = await cookiesFor(msg.url);
          try {
            const j = await bridge('/formats', {
              method: 'POST', headers: { 'Content-Type': 'application/json' },
              body: JSON.stringify({ url: msg.url, referer: msg.referer || '', userAgent: UA, cookies: ck.list }),
            }, settings, 40000);
            sendResponse(j);
          } catch (e) { sendResponse({ ok: false, error: String(e) }); }
          break;
        }
        case 'getMedia': sendResponse(await readMedia(msg.tabId)); break;
        case 'downloads': sendResponse(await getDownloads()); break;
        case 'action': {
          const j = await bridge('/action', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ id: msg.id, action: msg.action }) });
          tickProgress();
          sendResponse(j);
          break;
        }
        case 'download': sendResponse(await sendToApp({ ...msg, tabId: msg.tabId ?? sender.tab?.id, title: msg.title ?? sender.tab?.title })); break;
        case 'setSettings': await setSettings(msg.settings || {}); sendResponse({ ok: true }); break;
        case 'launchApp': {
          const settings = await getSettings();
          const app = await ensureApp(settings);
          sendResponse({ ok: !!app });
          break;
        }
        case 'pageMedia': {
          if (sender.tab) {
            for (const m of msg.items || []) {
              const kind = classify(m.url, m.type || '');
              if (kind || MEDIA_EXT.test(m.url)) addMedia(sender.tab.id, { url: m.url, type: m.type || '', size: 0, kind: kind || 'media', time: Date.now(), name: m.name });
            }
          }
          sendResponse({ ok: true });
          break;
        }
        default: sendResponse(null);
      }
    } catch (e) {
      sendResponse({ ok: false, error: String(e) });
    }
  })();
  return true;
});
