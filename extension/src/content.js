// Nehir içerik betiği: sayfadaki <video>/<audio>/<source> adreslerini bildirir ve oynatıcıların
// üstüne küçük bir indirme düğmesi koyar: tıklayınca Video / MP3 menüsü; "Video"da çözünürlük listesi.
(() => {
  'use strict';
  if (window.__nehirContent) return;
  window.__nehirContent = true;

  const api = globalThis.browser ?? globalThis.chrome;
  const I = globalThis.NehirI18n;
  const send = (msg) => { try { return api.runtime.sendMessage(msg); } catch { return Promise.resolve(null); } };

  // ---- Adres bildirimi ----
  const seen = new Set();
  function scan() {
    const items = [];
    document.querySelectorAll('video, audio, source').forEach((el) => {
      const url = el.currentSrc || el.src;
      if (!url || !/^https?:/i.test(url) || seen.has(url)) return;
      seen.add(url);
      items.push({ url, type: el.type || '' });
    });
    if (items.length) send({ type: 'pageMedia', items });
  }
  scan();
  new MutationObserver(() => scan()).observe(document.documentElement, { childList: true, subtree: true, attributes: true, attributeFilter: ['src'] });
  document.addEventListener('loadedmetadata', scan, true);
  document.addEventListener('playing', scan, true);

  // ---- İndirme düğmesi ----
  if (window.top !== window) return; // yalnızca ana çerçevede; iframe içindeki videolar sayfa adresiyle indirilir

  let enabled = true;
  // Çarpıyla kapatılan sayfa adresi. Yalnızca bu sayfa yüklemesi için geçerlidir (kalıcı değil):
  // sayfa yenilenince ya da başka videoya geçilince (adres değişince) düğme geri gelir.
  let closedFor = null;

  const box = document.createElement('div');
  box.style.cssText = 'all:initial;position:fixed;z-index:2147483647;display:none;';
  const root = box.attachShadow({ mode: 'closed' });
  root.innerHTML = `
    <style>
      *{box-sizing:border-box;font-family:system-ui,-apple-system,"Segoe UI",Roboto,sans-serif}
      .wrap{position:relative;width:34px;height:34px}
      .main{all:unset;box-sizing:border-box;width:34px;height:34px;border-radius:10px;cursor:pointer;display:grid;place-items:center;color:#fff;
        background:linear-gradient(135deg,#1257e6,#05b4d6);box-shadow:0 2px 10px rgba(0,0,0,.45);transition:transform .12s,filter .12s}
      .main:hover{transform:scale(1.07);filter:brightness(1.1)}
      .x{all:unset;box-sizing:border-box;position:absolute;top:-7px;right:-7px;width:18px;height:18px;border-radius:50%;cursor:pointer;display:grid;place-items:center;
        background:#0f1624;color:#c9d3e6;border:1px solid rgba(255,255,255,.18);opacity:0;transition:opacity .12s}
      .wrap:hover .x,.x:focus-visible{opacity:1}
      .x:hover{background:#e5484d;color:#fff;border-color:transparent}
      .menu{position:absolute;top:40px;right:0;min-width:236px;max-height:340px;overflow-y:auto;padding:6px;border-radius:12px;white-space:nowrap;
        background:rgba(15,22,36,.97);border:1px solid rgba(255,255,255,.12);box-shadow:0 10px 30px rgba(0,0,0,.5);display:none}
      .menu.open{display:block}
      .head{display:flex;align-items:center;gap:8px;font-size:11px;font-weight:700;letter-spacing:.06em;text-transform:uppercase;color:#7f8ca6;padding:6px 10px 4px}
      .back{all:unset;cursor:pointer;color:#9fb0cc;display:grid;place-items:center;width:20px;height:20px;border-radius:6px}
      .back:hover{background:rgba(255,255,255,.1);color:#fff}
      .item{all:unset;box-sizing:border-box;width:100%;display:flex;align-items:center;gap:10px;padding:9px 10px;border-radius:8px;cursor:pointer;color:#e8edf6;font-size:13px;font-weight:500}
      .item:hover{background:rgba(255,255,255,.09)}
      .item svg{flex:none;color:#4aa3ff}
      .item.audio svg{color:#ffb454}
      .item .sub{margin-left:auto;color:#8b97ad;font-size:12px;font-variant-numeric:tabular-nums;padding-left:14px}
      .item.best{font-weight:700}
      .note{color:#8b97ad;font-size:12px;padding:8px 10px}
      .spin{display:inline-block;width:12px;height:12px;border:2px solid #3b4a68;border-top-color:#4aa3ff;border-radius:50%;margin-right:8px;vertical-align:-2px;animation:r .8s linear infinite}
      @keyframes r{to{transform:rotate(360deg)}}
      .toast{position:absolute;top:3px;right:42px;padding:7px 12px;border-radius:9px;font-size:12.5px;font-weight:600;color:#fff;white-space:nowrap;
        background:rgba(15,22,36,.96);border:1px solid rgba(255,255,255,.14);box-shadow:0 4px 16px rgba(0,0,0,.45);display:none}
      .toast.show{display:block}
      .toast.ok{border-color:#2ea043;color:#7ee2a0}
      .toast.bad{border-color:#e5484d;color:#ff9aa5}
    </style>
    <div class="wrap">
      <div class="toast"></div>
      <button class="main" type="button" aria-label="Nehir">
        <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"><path d="M12 3v12m0 0l-5-5m5 5l5-5M5 21h14"/></svg>
      </button>
      <button class="x" type="button" aria-label="×">
        <svg width="9" height="9" viewBox="0 0 12 12"><path d="M2 2l8 8M10 2l-8 8" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/></svg>
      </button>
      <div class="menu"></div>
    </div>`;
  const $ = (s) => root.querySelector(s);
  const menu = $('.menu');
  const mainBtn = $('.main');
  const toastEl = $('.toast');
  document.documentElement.appendChild(box);

  const ICON_DL = '<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"><path d="M12 3v12m0 0l-5-5m5 5l5-5M5 21h14"/></svg>';
  const ICON_OK = '<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><path d="M5 12.5l4.5 4.5L19 7.5"/></svg>';
  const ICON_VIDEO = '<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="5" width="13" height="14" rx="2"/><path d="M16 10l5-3v10l-5-3"/></svg>';
  const ICON_MUSIC = '<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M9 18V5l11-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="17" cy="16" r="3"/></svg>';
  const ICON_BACK = '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"><path d="M15 5l-7 7 7 7"/></svg>';

  const el = (html) => { const d = document.createElement('div'); d.innerHTML = html.trim(); return d.firstElementChild; };
  const esc = (s) => String(s).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
  const fmtSize = (n) => {
    if (!(n > 0)) return '';
    const u = ['B', 'KB', 'MB', 'GB']; let i = 0;
    while (n >= 1024 && i < u.length - 1) { n /= 1024; i++; }
    return '~' + (n >= 100 || i === 0 ? n.toFixed(0) : n.toFixed(1)) + ' ' + u[i];
  };

  // ---- Menü görünümleri ----
  function showMain() {
    menu.innerHTML = `
      <div class="head">${esc(I.t('ovTitle'))}</div>
      <button class="item video" type="button" data-act="video">${ICON_VIDEO}<span>${esc(I.t('ovVideo'))}</span></button>
      <button class="item audio" type="button" data-act="audio">${ICON_MUSIC}<span>${esc(I.t('ovAudio'))}</span></button>`;
  }

  function currentTarget() {
    const src = current?.currentSrc || current?.src || '';
    // blob: ve parçalı akışlarda sayfa adresi yt-dlp'ye verilir; düz dosyada doğrudan adres.
    const direct = /^https?:/i.test(src) && /\.(mp4|webm|mkv|mov|m4v|mp3|m4a|ogg)(\?|#|$)/i.test(src);
    return { direct, url: direct ? src : location.href };
  }

  let formatsToken = 0;
  async function showQuality() {
    const token = ++formatsToken;
    menu.innerHTML = `
      <div class="head"><button class="back" type="button" data-act="back" aria-label="${esc(I.t('ovBack'))}">${ICON_BACK}</button><span>${esc(I.t('ovQuality'))}</span></div>
      <button class="item best" type="button" data-act="q" data-q="best">${ICON_VIDEO}<span>${esc(I.t('ovBest'))}</span></button>
      <div class="note"><i class="spin"></i>${esc(I.t('ovLoading'))}</div>`;
    const { url } = currentTarget();
    const res = await send({ type: 'formats', url, referer: location.href });
    if (token !== formatsToken || !menu.classList.contains('open')) return; // kullanıcı başka yere geçti
    const note = menu.querySelector('.note');
    if (!note) return;
    let options = [];
    let message = '';
    if (res?.ok && Array.isArray(res.options) && res.options.length) {
      options = res.options;
    } else {
      // Liste alınamadı (yt-dlp yok, adres desteklenmiyor…): yaklaşık, en çok kullanılan çözünürlükler.
      options = [1080, 720, 480, 360].map((h) => ({ height: h, size: 0 }));
      message = res?.error === 'ytdlp_missing' ? I.t('ovNoYtdlp') : I.t('ovApprox');
    }
    note.remove();
    for (const o of options) {
      menu.append(el(`<button class="item" type="button" data-act="q" data-q="${o.height}">${ICON_VIDEO}<span>${o.height}p</span><span class="sub">${esc(fmtSize(o.size))}</span></button>`));
    }
    if (message) menu.append(el(`<div class="note">${esc(message)}</div>`));
  }

  function toast(text, kind) {
    toastEl.textContent = text;
    toastEl.className = 'toast show ' + kind;
    clearTimeout(toast.t);
    toast.t = setTimeout(() => { toastEl.className = 'toast'; }, 2600);
  }

  async function startDownload({ audio, quality }) {
    const { direct, url } = currentTarget();
    closeMenu();
    const res = await send({
      type: 'download', url, referer: location.href, kind: direct && !audio ? 'file' : 'video', audio, quality, title: document.title,
    });
    const ok = !!(res && res.ok);
    toast(ok ? I.t('ovSent') : I.t('ovFailed'), ok ? 'ok' : 'bad');
    mainBtn.innerHTML = ok ? ICON_OK : ICON_DL;
    setTimeout(() => { mainBtn.innerHTML = ICON_DL; }, 1800);
  }

  const labels = () => {
    $('.x').title = I.t('ovHide');
    mainBtn.title = I.t('ovTitle');
  };

  Promise.resolve(I.ready).then(() => send({ type: 'overlayState' })).then((s) => {
    if (s?.lang) I.setLang(s.lang);
    labels();
    enabled = s?.enabled ?? true;
    if (!enabled) box.style.display = 'none';
  });
  try {
    api.storage.onChanged.addListener((ch, area) => {
      if (area !== 'local') return;
      if (ch.settings) { enabled = ch.settings.newValue?.overlay ?? true; if (!enabled) box.style.display = 'none'; }
      if (ch.appLanguage) { I.setLang(ch.appLanguage.newValue); labels(); }
    });
  } catch {}

  let current = null;
  let hideTimer = null;
  const closeMenu = () => { menu.classList.remove('open'); formatsToken++; };

  function place(video) {
    const r = video.getBoundingClientRect();
    if (r.width < 240 || r.height < 140 || r.bottom < 0 || r.top > innerHeight) { box.style.display = 'none'; closeMenu(); return; }
    box.style.display = 'block';
    box.style.top = Math.max(10, r.top + 12) + 'px';
    box.style.left = Math.max(10, Math.min(innerWidth - 46, r.right - 46)) + 'px';
  }
  function show(video) {
    if (!enabled) return;
    if (closedFor !== null) {
      if (closedFor === location.href) return; // bu video için kapatıldı
      closedFor = null;                          // başka videoya/sayfaya geçildi: geri gelsin
    }
    current = video;
    clearTimeout(hideTimer);
    place(video);
  }
  function scheduleHide() {
    clearTimeout(hideTimer);
    hideTimer = setTimeout(() => { if (!menu.classList.contains('open')) { box.style.display = 'none'; current = null; } }, 1800);
  }

  document.addEventListener('mouseover', (e) => {
    const v = e.target instanceof Element ? e.target.closest('video') : null;
    if (v) show(v);
  }, true);
  document.addEventListener('mouseout', (e) => { if (e.target instanceof Element && e.target.closest('video')) scheduleHide(); }, true);
  box.addEventListener('mouseenter', () => clearTimeout(hideTimer));
  box.addEventListener('mouseleave', () => scheduleHide());
  addEventListener('scroll', () => { if (current) place(current); }, true);
  addEventListener('resize', () => { if (current) place(current); });
  document.addEventListener('click', (e) => { if (!e.composedPath().includes(box)) closeMenu(); }, true);

  mainBtn.addEventListener('click', (e) => {
    e.preventDefault(); e.stopPropagation();
    if (menu.classList.contains('open')) { closeMenu(); return; }
    showMain();
    menu.classList.add('open');
  });
  $('.x').addEventListener('click', (e) => {
    e.preventDefault(); e.stopPropagation();
    closedFor = location.href; // kalıcı değil: yenileyince ya da başka videoya geçince geri gelir
    box.style.display = 'none';
    closeMenu();
  });
  menu.addEventListener('click', (e) => {
    const item = e.target instanceof Element ? e.target.closest('[data-act]') : null;
    if (!item || !current) return;
    e.preventDefault(); e.stopPropagation();
    const act = item.dataset.act;
    if (act === 'back') showMain();
    else if (act === 'audio') startDownload({ audio: true });
    else if (act === 'video') {
      // Düz dosyalarda (mp4 vb.) çözünürlük seçeneği yoktur: doğrudan indir.
      if (currentTarget().direct) startDownload({ audio: false });
      else showQuality();
    } else if (act === 'q') startDownload({ audio: false, quality: item.dataset.q });
  });
})();
