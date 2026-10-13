import { api, isTauri } from './api.js';
import { icon, hydrateIcons } from './icons.js';
import { t, setLanguage, applyI18n, getLanguage } from './i18n.js';

const $ = (sel, root = document) => root.querySelector(sel);
const el = (tag, cls, html) => { const e = document.createElement(tag); if (cls) e.className = cls; if (html != null) e.innerHTML = html; return e; };
const esc = (s) => String(s ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));

// ---------- Durum ----------
const downloads = new Map();
const expanded = new Set();
let filter = 'all';
let query = '';
let settings = null;
let status = null;
const rows = new Map();

const CATS = ['video', 'music', 'document', 'archive', 'program', 'image', 'other'];
const ACTIVE = new Set(['probing', 'downloading', 'processing']);

// ---------- Biçimlendirme ----------
function fmtSize(n) {
  if (!(n > 0)) return '—';
  const u = ['B', 'KB', 'MB', 'GB', 'TB'];
  let i = 0;
  while (n >= 1024 && i < u.length - 1) { n /= 1024; i++; }
  return (n >= 100 || i === 0 ? n.toFixed(0) : n.toFixed(1)) + ' ' + u[i];
}
const fmtSpeed = (b) => (b > 0 ? fmtSize(b) + t('speedUnit') : '');
function fmtEta(sec) {
  if (!isFinite(sec) || sec <= 0) return '';
  const h = Math.floor(sec / 3600), m = Math.floor((sec % 3600) / 60), s = Math.floor(sec % 60);
  const [H, M, S] = getLanguageShort() === 'tr' ? ['sa', 'dk', 'sn'] : ['h', 'm', 's'];
  return h ? `${h}${H} ${m}${M}` : m ? `${m}${M} ${s}${S}` : `${s}${S}`;
}
const getLanguageShort = () => getLanguage();
// yt-dlp ve motor hatalarını anlaşılır Türkçe/İngilizce metne çevirir; ham metin ipucunda kalır.
const DRM_HOSTS = /(netflix|primevideo|disneyplus|hbomax|max\.com|spotify|tv\.apple|hulu|paramountplus)/i;
function errorText(d) {
  const e = String(d.error || '');
  if (e === 'ytdlp_missing') return t('ytdlpMissing');
  if (/unsupported url/i.test(e)) return DRM_HOSTS.test(d.url) ? t('err.drm') : t('err.unsupported');
  if (/http error 403|forbidden|sign in|login/i.test(e)) return t('err.forbidden');
  return e.length > 160 ? e.slice(0, 157) + '…' : e;
}
const updatableError = (d) => !/unsupported url|ytdlp_missing/i.test(String(d.error || ''));

function stageText(stage) {
  if (!stage) return '';
  if (stage.startsWith('format:')) return `${t('stage.format')}: ${stage.slice(7)}`;
  return t('stage.' + stage);
}

// ---------- Filtreleme ----------
function matches(d) {
  if (query && !(`${d.file_name} ${d.url}`.toLowerCase().includes(query))) return false;
  if (filter === 'all') return true;
  if (filter === 'active') return ACTIVE.has(d.state);
  if (filter === 'queued') return ['queued', 'paused', 'waiting'].includes(d.state);
  if (filter === 'completed') return d.state === 'completed';
  if (filter.startsWith('cat:')) return d.category === filter.slice(4);
  return true;
}
const sorted = () => [...downloads.values()].filter(matches).sort((a, b) => b.created_at - a.created_at);

// ---------- Kenar çubuğu ----------
function renderNav() {
  const all = [...downloads.values()];
  const count = (f) => all.filter((d) => {
    if (f === 'all') return true;
    if (f === 'active') return ACTIVE.has(d.state);
    if (f === 'queued') return ['queued', 'paused', 'waiting'].includes(d.state);
    if (f === 'completed') return d.state === 'completed';
    return d.category === f.slice(4);
  }).length;
  const items = [
    ['all', 'all', t('nav.all')], ['active', 'bolt', t('nav.active')], ['queued', 'pause', t('nav.queued')], ['completed', 'check', t('nav.completed')],
    'sep',
    ...CATS.map((c) => ['cat:' + c, c === 'document' ? 'doc' : c, t('cat.' + c)]),
  ];
  const nav = $('#nav');
  nav.innerHTML = '';
  for (const it of items) {
    if (it === 'sep') { nav.append(el('div', 'nav-sep')); continue; }
    const [key, ic, label] = it;
    const b = el('button', 'nav-item' + (filter === key ? ' active' : ''), `<span class="ico">${icon(ic)}</span><span>${esc(label)}</span><span class="count">${count(key) || ''}</span>`);
    b.onclick = () => { filter = key; renderNav(); renderList(); };
    nav.append(b);
  }
}

// ---------- Liste ----------
function buildRow(d) {
  const row = el('div', 'row');
  row.dataset.id = d.id;
  row.innerHTML = `
    <div class="thumb ${esc(d.category)}"></div>
    <div class="name"></div>
    <div class="actions"></div>
    <div class="progress"><div></div></div>
    <div class="meta" style="grid-column:2"></div>`;
  row.addEventListener('click', (e) => {
    if (e.target.closest('.actions')) return;
    expanded.has(d.id) ? expanded.delete(d.id) : expanded.add(d.id);
    updateRow(downloads.get(d.id));
  });
  return row;
}

function actionButtons(d) {
  const b = [];
  const add = (ic, title, fn, cls = '') => b.push({ ic, title, fn, cls });
  if (ACTIVE.has(d.state) || d.state === 'queued' || d.state === 'waiting') add('pause', t('pause'), () => api.pause(d.id));
  if (d.state === 'paused' || d.state === 'failed') add('play', t('resume'), () => api.resume(d.id));
  if (d.state === 'completed') { add('open', t('open'), () => api.openFile(d.id)); add('folder', t('folder'), () => api.openFolder(d.id)); }
  if (!['completed', 'cancelled'].includes(d.state)) add('x', t('cancel'), () => api.cancel(d.id), 'danger');
  add('trash', t('delete'), () => api.remove(d.id, d.state === 'completed' && confirmDeleteFile(d)), 'danger');
  return b;
}
const confirmDeleteFile = () => false; // dosyayı silmek için ayrı onay istenebilir; varsayılan: yalnızca listeden kaldır

function updateRow(d) {
  if (!d) return;
  let row = rows.get(d.id);
  if (!row) { row = buildRow(d); rows.set(d.id, row); }
  row.querySelector('.thumb').className = `thumb ${d.category}`;
  row.querySelector('.thumb').innerHTML = icon(d.category === 'document' ? 'doc' : d.category, 22);
  const nameEl = row.querySelector('.name');
  nameEl.textContent = d.file_name || d.url;
  nameEl.title = d.url;

  const pct = d.total_bytes > 0 ? Math.min(100, (d.received_bytes / d.total_bytes) * 100) : 0;
  const prog = row.querySelector('.progress');
  prog.className = 'progress' + (d.state === 'completed' ? ' done' : d.state === 'failed' ? ' bad' : (d.state === 'paused' || d.state === 'waiting') ? ' paused' : (ACTIVE.has(d.state) && d.total_bytes <= 0) ? ' indeterminate' : '');
  prog.firstElementChild.style.width = (d.state === 'completed' ? 100 : pct) + '%';
  prog.hidden = d.state === 'cancelled';

  const eta = d.speed > 0 && d.total_bytes > 0 ? fmtEta((d.total_bytes - d.received_bytes) / d.speed) : '';
  const stateLabel = d.state === 'processing' || d.state === 'downloading' ? (stageText(d.stage) || t('state.' + d.state)) : t('state.' + d.state);
  const parts = [`<span class="state ${d.state}">${esc(stateLabel)}</span>`];
  if (d.state === 'completed') parts.push(esc(fmtSize(d.total_bytes)));
  else if (d.total_bytes > 0) parts.push(`${esc(fmtSize(d.received_bytes))} / ${esc(fmtSize(d.total_bytes))} (${pct.toFixed(0)}%)`);
  else if (d.received_bytes > 0) parts.push(esc(fmtSize(d.received_bytes)));
  if (ACTIVE.has(d.state) && d.speed > 0) parts.push(esc(fmtSpeed(d.speed)));
  if (eta) parts.push(`${esc(eta)} ${esc(t('eta'))}`);
  if (d.state === 'failed' && d.error) parts.push(`<span class="err-text" title="${esc(d.error)}">${esc(errorText(d))}</span>`);
  if (d.state === 'waiting') parts.push(`<span title="${esc(d.error || '')}">${esc(t('err.waiting'))}</span>`);
  row.querySelector('.meta').innerHTML = parts.join('<span>·</span>');
  if (d.state === 'failed' && d.kind === 'video' && updatableError(d)) {
    const missing = d.error === 'ytdlp_missing';
    const label = missing ? t('settings.ytdlpInstall') : t('ytdlpUpdate');
    const b = el('button', missing ? 'primary' : '', esc(label));
    b.style.cssText = 'padding:2px 10px;font-size:12px';
    b.onclick = async (e) => {
      e.stopPropagation(); b.disabled = true; b.textContent = t('settings.installing');
      try { await api.installYtdlp(); toast(t('toast.ytdlpInstalled'), 'ok'); await api.resume(d.id); } catch (err) { toast(`${t('toast.error')}: ${err}`, 'bad'); b.disabled = false; b.textContent = label; }
    };
    row.querySelector('.meta').append(b);
  }

  const actions = row.querySelector('.actions');
  actions.innerHTML = '';
  for (const a of actionButtons(d)) {
    const btn = el('button', a.cls, icon(a.ic, 16));
    if (a.title) btn.title = a.title;
    btn.onclick = (e) => { e.stopPropagation(); a.fn(); };
    actions.append(btn);
  }

  let detail = row.querySelector('.detail');
  if (expanded.has(d.id)) {
    if (!detail) { detail = el('div', 'detail'); row.append(detail); }
    const segs = (d.segments || []).map((s) => {
      const len = s.end - s.start + 1;
      return `<i style="flex:${len}"><b style="width:${Math.min(100, (s.received / len) * 100)}%"></b></i>`;
    }).join('');
    detail.innerHTML = `
      <div><span class="k">${esc(t('detail.url'))}</span><span class="val">${esc(d.url)}</span></div>
      <div><span class="k">${esc(t('detail.path'))}</span><span class="val">${esc(d.dir + '/' + d.file_name)}</span></div>
      <div>${d.kind === 'video' ? 'yt-dlp' : d.supports_ranges ? `${esc(t('detail.ranges'))} · ${d.segments.length || d.segment_count} ${esc(t('parts'))}` : esc(t('detail.noRanges'))}</div>
      ${segs ? `<div><span class="k">${esc(t('detail.segments'))}</span></div><div class="segs">${segs}</div>` : ''}`;
  } else if (detail) detail.remove();
}

function renderList() {
  const list = $('#list');
  const items = sorted();
  $('#empty').hidden = downloads.size > 0;
  list.hidden = downloads.size === 0;
  const want = new Set(items.map((d) => d.id));
  for (const [id, row] of rows) if (!want.has(id)) { row.remove(); if (!downloads.has(id)) rows.delete(id); }
  let prev = null;
  for (const d of items) {
    updateRow(d);
    const row = rows.get(d.id);
    if (row.previousElementSibling !== prev || row.parentElement !== list) list.insertBefore(row, prev ? prev.nextSibling : list.firstChild);
    prev = row;
  }
  updateTopbar();
}

function updateTopbar() {
  const total = [...downloads.values()].filter((d) => ACTIVE.has(d.state)).reduce((a, d) => a + (d.speed || 0), 0);
  $('#total-speed').textContent = total > 0 ? '↓ ' + fmtSpeed(total) : '';
}

// ---------- Bildirimler ----------
function toast(text, kind = '', action) {
  const box = el('div', 'toast ' + kind, `<span>${esc(text)}</span>`);
  if (action) { const b = el('button', 'primary', esc(action.label)); b.onclick = () => { action.fn(); box.remove(); }; box.append(b); }
  $('#toasts').append(box);
  setTimeout(() => box.remove(), action ? 9000 : 4500);
}

// ---------- Ekleme penceresi ----------
const VIDEO_HOSTS = /(^|\.)(youtube\.com|youtu\.be|vimeo\.com|dailymotion\.com|twitch\.tv|tiktok\.com|instagram\.com|facebook\.com|fb\.watch|twitter\.com|x\.com|reddit\.com|bilibili\.com)$/i;
function looksLikeVideo(u) {
  try { const x = new URL(u); return VIDEO_HOSTS.test(x.hostname) || /\.(m3u8|mpd)(\?|$)/i.test(x.pathname); } catch { return false; }
}
const isUrl = (s) => /^https?:\/\/\S+$/i.test(s.trim());

async function openAddDialog(prefill) {
  const dlg = $('#dlg-add');
  let text = prefill || '';
  if (!text) {
    try { const c = (await api.readClipboard())?.trim?.() || ''; if (isUrl(c.split('\n')[0])) text = c; } catch {}
  }
  $('#add-urls').value = text;
  $('#add-video').checked = text.split('\n').some(looksLikeVideo);
  fillQuality();
  syncQualityField();
  dlg.showModal();
  $('#add-urls').focus();
  $('#add-urls').select();
}
function fillQuality() {
  const sel = $('#add-quality');
  sel.innerHTML = '';
  for (const q of ['default', 'best', '2160', '1440', '1080', '720', '480', 'audio']) sel.append(new Option(t('q.' + q), q));
}
const syncQualityField = () => { $('#add-quality-field').hidden = !$('#add-video').checked; };
$('#add-urls').addEventListener('input', () => { $('#add-video').checked = $('#add-urls').value.split('\n').some(looksLikeVideo); syncQualityField(); });
$('#add-video').addEventListener('change', syncQualityField);
$('#add-cancel').onclick = () => $('#dlg-add').close();
$('#form-add').addEventListener('submit', async () => {
  const urls = $('#add-urls').value.split('\n').map((s) => s.trim()).filter(isUrl);
  if (!urls.length) return;
  const q = $('#add-quality').value;
  try { await api.add(urls, $('#add-video').checked, $('#add-video').checked && q !== 'default' ? q : null); toast(t('toast.added'), 'ok'); } catch (e) { toast(`${t('toast.error')}: ${e}`, 'bad'); }
});

// ---------- Ayarlar ----------
function settingsField(label, control, hint) {
  const f = el('div', 'field');
  const l = el('label', null, esc(label));
  f.append(l, control);
  if (hint) f.append(el('div', 'hint', esc(hint)));
  return f;
}
const checkbox = (id, label, checked) => { const w = el('label', 'check', `<input type="checkbox" id="${id}" ${checked ? 'checked' : ''}><span>${esc(label)}</span>`); return w; };

async function openSettings() {
  const s = { ...(await api.getSettings()) };
  status = await api.status();
  const body = $('#settings-body');
  body.innerHTML = '';
  body.append(el('h3', null, esc(t('settings.title'))));

  body.append(el('h4', null, esc(t('settings.general'))));
  const lang = el('select');
  [['auto', t('settings.langAuto')], ['tr', 'Türkçe'], ['en', 'English']].forEach(([v, n]) => lang.append(new Option(n, v, false, (['tr', 'en'].includes(s.language) ? s.language : 'auto') === v)));
  body.append(settingsField(t('settings.language'), lang));
  const dirIn = el('input'); dirIn.type = 'text'; dirIn.value = s.download_dir;
  const pick = el('button', null, esc(t('settings.choose'))); pick.type = 'button';
  pick.onclick = async () => { const p = await api.pickFolder(); if (p) dirIn.value = p; };
  const pair = el('div', 'pair'); pair.append(dirIn, pick);
  body.append(settingsField(t('settings.dir'), pair));
  body.append(checkbox('s-cat', t('settings.categorize'), s.categorize));
  body.append(checkbox('s-clip', t('settings.clipboard'), s.monitor_clipboard));
  body.append(checkbox('s-notif', t('settings.notifications'), s.notifications));
  body.append(checkbox('s-tray', t('settings.tray'), s.close_to_tray));
  body.append(checkbox('s-auto', t('settings.autostart'), s.autostart));

  body.append(el('h4', null, esc(t('settings.downloads'))));
  const range = (id, min, max, val) => el('div', 'range', `<input type="range" id="${id}" min="${min}" max="${max}" value="${val}"><output>${val}</output>`);
  const conn = range('s-conn', 1, 32, s.max_connections);
  const conc = range('s-conc', 1, 10, s.max_concurrent);
  for (const r of [conn, conc]) r.firstElementChild.oninput = (e) => { r.lastElementChild.textContent = e.target.value; };
  body.append(settingsField(t('settings.connections'), conn));
  body.append(settingsField(t('settings.concurrent'), conc));
  const speed = el('input'); speed.type = 'number'; speed.min = 0; speed.value = s.speed_limit_kbps;
  body.append(settingsField(t('settings.speed'), speed));
  const cap = el('textarea'); cap.rows = 3; cap.value = s.capture_extensions.join(' ');
  body.append(settingsField(t('settings.capture'), cap, t('settings.captureHint')));
  const port = el('input'); port.type = 'number'; port.min = 1024; port.max = 65535; port.value = s.port;
  body.append(settingsField(t('settings.port'), port, t('settings.portHint')));

  body.append(el('h4', null, esc(t('settings.video'))));
  const q = el('select');
  [['best', t('settings.qualityBest')], ['2160', '4K (2160p)'], ['1440', '1440p'], ['1080', '1080p'], ['720', '720p'], ['480', '480p'], ['audio', t('settings.qualityAudio')]]
    .forEach(([v, n]) => q.append(new Option(n, v, false, s.video_quality === v)));
  body.append(settingsField(t('settings.quality'), q));
  const yt = el('div', 'field inline');
  const ytBadge = el('span', 'badge ' + (status.ytdlp ? 'ok' : 'bad'), `${icon(status.ytdlp ? 'check' : 'alert', 14)} ${esc(t('settings.ytdlp'))}: ${esc(status.ytdlp ? t('settings.ytdlpOk') : t('settings.ytdlpMissing'))}`);
  yt.append(ytBadge);
  if (!status.ytdlp) {
    const inst = el('button', null, esc(t('settings.ytdlpInstall'))); inst.type = 'button';
    inst.onclick = async () => {
      inst.disabled = true; inst.textContent = t('settings.installing');
      try { await api.installYtdlp(); toast(t('toast.ytdlpInstalled'), 'ok'); status = await api.status(); openSettings(); }
      catch (e) { toast(`${t('toast.error')}: ${e}`, 'bad'); inst.disabled = false; inst.textContent = t('settings.ytdlpInstall'); }
    };
    yt.append(inst);
  }
  body.append(yt);

  const buttons = el('div', 'buttons');
  const cancel = el('button', null, esc(t('cancel'))); cancel.type = 'button'; cancel.onclick = () => $('#dlg-settings').close();
  const save = el('button', 'primary', esc(t('save'))); save.type = 'button';
  save.onclick = async () => {
    const next = {
      ...s,
      language: lang.value, download_dir: dirIn.value.trim() || s.download_dir,
      categorize: $('#s-cat').checked, monitor_clipboard: $('#s-clip').checked, notifications: $('#s-notif').checked,
      close_to_tray: $('#s-tray').checked, autostart: $('#s-auto').checked,
      max_connections: Number($('#s-conn').value), max_concurrent: Number($('#s-conc').value),
      speed_limit_kbps: Math.max(0, Number(speed.value) || 0), port: Number(port.value) || s.port,
      capture_extensions: cap.value.split(/[\s,]+/).filter(Boolean), video_quality: q.value,
    };
    try { settings = await api.setSettings(next); applyLanguage(); $('#dlg-settings').close(); } catch (e) { toast(`${t('toast.error')}: ${e}`, 'bad'); }
  };
  buttons.append(cancel, save);
  body.append(buttons);
  if (!$('#dlg-settings').open) $('#dlg-settings').showModal();
}

// ---------- Ağ optimizasyonu ----------
const humanBytes = (n) => (n >= 1048576 ? `${+(n / 1048576).toFixed(1)} MB` : n >= 1024 ? `${Math.round(n / 1024)} KB` : `${n} B`);
function humanVal(id, v) {
  if (v == null) return '—';
  if (['rmem_max', 'wmem_max'].includes(id) && /^\d+$/.test(v)) return humanBytes(Number(v));
  if (['tcp_rmem', 'tcp_wmem'].includes(id)) { const m = Number(String(v).split(' ').pop()); return Number.isFinite(m) ? `${t('net.col.max')} ${humanBytes(m)}` : v; }
  if (id === 'slow_start_idle' || id === 'mtu_probing') return v === '0' ? t('net.off') : t('net.on');
  return v;
}

async function netAction(fn, okKey, buttons) {
  buttons.forEach((b) => { b.disabled = true; });
  try { await fn(); toast(t(okKey), 'ok'); }
  catch (e) {
    const msg = String(e);
    toast(msg === 'cancelled' ? t('net.cancelled') : msg === 'helper_missing' ? t('net.helperMissing') : `${t('toast.error')}: ${msg}`, msg === 'cancelled' ? '' : 'bad');
  }
  await openNetDialog();
}

async function openNetDialog() {
  const r = await api.netInspect();
  const lang = getLanguage();
  const body = $('#net-body');
  body.innerHTML = '';
  body.append(el('h3', null, esc(t('net.title'))), el('p', 'hint', esc(t('net.intro'))));

  body.append(el('h4', null, esc(t('net.connections'))));
  for (const i of r.ifaces) {
    const detail = [i.driver, i.up ? t('net.up') : t('net.down'), i.speed_mbps ? `${i.speed_mbps} Mb/s` : null, `MTU ${i.mtu}`].filter(Boolean).join(' · ');
    const ic = i.kind === 'wifi' ? 'wifi' : i.kind === 'vpn' ? 'link' : 'plug';
    body.append(el('div', 'browser-row', `<span class="ico">${icon(ic)}</span><div><strong>${esc(i.name)}</strong><div class="hint" style="margin:0">${esc(detail)}</div></div>${i.default_route ? `<span class="badge ok" style="margin-left:auto">${esc(t('net.defaultRoute'))}</span>` : ''}`));
  }
  if (r.vpn_in_route) body.append(el('div', 'net-warn', `${icon('alert', 16)}<span>${esc(t('net.vpnWarn'))}</span>`));

  body.append(el('h4', null, esc(t('net.tcp'))));
  const table = el('table', 'net-table');
  table.innerHTML = `<thead><tr><th>${esc(t('net.col.setting'))}</th><th>${esc(t('net.col.now'))}</th><th>${esc(t('net.col.rec'))}</th><th></th></tr></thead>`;
  const tb = el('tbody');
  for (const it of r.items) {
    const tr = el('tr', it.ok ? '' : 'bad');
    tr.title = t('nh.' + it.id);
    tr.innerHTML = `<td>${esc(t('ni.' + it.id))}</td><td>${esc(humanVal(it.id, it.current))}</td><td>${esc(humanVal(it.id, it.recommended))}</td><td class="st">${it.ok ? icon('check', 16) : '!'}</td>`;
    tb.append(tr);
  }
  table.append(tb);
  body.append(table);
  body.append(el('p', r.tcp_ok ? 'net-ok' : 'hint', esc(r.tcp_ok ? t('net.allOk') : t('net.someBad'))));

  const tcpRow = el('div', 'buttons left');
  if (!r.helper_installed) body.append(el('p', 'net-warn-text', esc(t('net.helperMissing'))));
  else {
    if (!r.tcp_ok) { const b = el('button', 'primary', esc(t('net.apply'))); b.onclick = () => netAction(() => api.netApply(true, false), 'net.done', [b]); tcpRow.append(b); }
    if (r.tcp_applied) { const b = el('button', null, esc(t('net.revert'))); b.onclick = () => netAction(() => api.netRevert(true, false), 'net.reverted', [b]); tcpRow.append(b); }
    if (r.tcp_applied) tcpRow.append(el('span', 'badge ok', esc(t('net.applied'))));
    body.append(tcpRow);
  }

  if (r.wifi_present) {
    body.append(el('h4', null, esc(t('net.wifi'))), el('p', 'hint', esc(t('net.wifiText'))));
    const w = el('div', 'buttons left');
    if (r.helper_installed) {
      if (!r.wifi_applied) { const b = el('button', null, esc(t('net.wifiApply'))); b.onclick = () => netAction(() => api.netApply(false, true), 'net.done', [b]); w.append(b); }
      else { const b = el('button', null, esc(t('net.revert'))); b.onclick = () => netAction(() => api.netRevert(false, true), 'net.reverted', [b]); w.append(b, el('span', 'badge ok', esc(t('net.applied')))); }
      body.append(w, el('p', 'hint', esc(t('net.wifiNote'))));
    }
  }
  body.append(el('p', 'hint', esc(t('net.foot'))));
  const buttons = el('div', 'buttons');
  const close = el('button', null, esc(t('close'))); close.type = 'button'; close.onclick = () => $('#dlg-net').close();
  buttons.append(close);
  body.append(buttons);
  void lang;
  if (!$('#dlg-net').open) $('#dlg-net').showModal();
}

// ---------- Tarayıcı bağlantısı ----------
const BROWSERS = [['chrome', 'Chrome'], ['brave', 'Brave'], ['edge', 'Edge'], ['chromium', 'Chromium'], ['vivaldi', 'Vivaldi'], ['opera', 'Opera'], ['firefox', 'Firefox']];
const connected = (ts) => ts && Date.now() / 1000 - ts < 180;

async function openBrowserDialog() {
  status = await api.status();
  const body = $('#browser-body');
  body.innerHTML = '';
  body.append(el('h3', null, esc(t('browser.title'))), el('p', 'hint', esc(t('browser.intro'))));
  const seen = status.extensions_seen || {};
  for (const [key, name] of BROWSERS) {
    if (!seen[key] && !['chrome', 'firefox'].includes(key)) continue;
    const on = connected(seen[key]);
    body.append(el('div', 'browser-row', `<strong>${esc(name)}</strong><span class="badge ${on ? 'ok' : ''}" style="margin-left:auto">${esc(on ? t('browser.connected') : t('browser.waiting'))}</span>`));
  }
  const steps = el('div', 'steps');
  steps.append(el('h4', null, esc(t('browser.step1'))));
  steps.append(el('div', 'path', esc(status.extension_path || '—')));
  const copy = el('button', null, esc(t('browser.copy'))); copy.type = 'button'; copy.style.marginTop = '8px';
  copy.onclick = () => api.copyText(status.extension_path || '');
  steps.append(copy);
  steps.append(el('h4', null, esc(t('browser.step2'))), el('p', null, esc(t('browser.step2text'))));
  steps.append(el('h4', null, esc(t('browser.step3'))), el('p', null, esc(t('browser.step3text'))));
  const reg = el('button', 'primary', esc(t('browser.register'))); reg.type = 'button';
  const result = el('div', 'hint');
  reg.onclick = async () => {
    try { const r = await api.registerHost(); result.textContent = `${t('browser.registered')}: ${r.join(', ') || '—'}`; toast(t('toast.hostRegistered'), 'ok'); }
    catch (e) { toast(`${t('toast.error')}: ${e}`, 'bad'); }
  };
  steps.append(reg, result);
  body.append(steps);
  const buttons = el('div', 'buttons');
  const close = el('button', null, esc(t('close'))); close.type = 'button'; close.onclick = () => $('#dlg-browser').close();
  buttons.append(close);
  body.append(buttons);
  if (!$('#dlg-browser').open) $('#dlg-browser').showModal();
}

async function refreshBrowserDot() {
  try { status = await api.status(); } catch { return; }
  const any = Object.values(status.extensions_seen || {}).some(connected);
  $('#browser-dot').classList.toggle('on', any);
}

// ---------- Başlangıç ----------
async function syncLanguage() {
  try { status = await api.status(); setLanguage(status.language); } catch { setLanguage('tr'); }
}

async function applyLanguage() {
  await syncLanguage();
  applyI18n();
  hydrateIcons();
  renderNav();
  for (const d of downloads.values()) updateRow(d);
  renderList();
}

async function main() {
  hydrateIcons();
  settings = await api.getSettings();
  await syncLanguage();
  applyI18n();
  for (const d of await api.list()) downloads.set(d.id, d);
  renderNav();
  renderList();

  await api.on('download-updated', (d) => {
    const isNew = !downloads.has(d.id);
    const prev = downloads.get(d.id);
    downloads.set(d.id, d);
    if (isNew || prev.state !== d.state || prev.category !== d.category) { renderNav(); renderList(); } else { updateRow(d); updateTopbar(); }
  });
  await api.on('download-removed', (id) => { downloads.delete(id); expanded.delete(id); rows.get(id)?.remove(); rows.delete(id); renderNav(); renderList(); });
  await api.on('download-completed', (d) => { if (settings?.notifications) toast(`${t('toast.completed')}: ${d.file_name}`, 'ok'); });
  await api.on('settings-changed', (s) => { settings = s; applyLanguage(); });
  await api.on('clipboard-url', (url) => toast(`${t('toast.clipboard')}`, '', { label: t('add'), fn: () => openAddDialog(url) }));
  await api.on('open-add-dialog', () => openAddDialog());

  $('#btn-add').onclick = () => openAddDialog();
  $('#btn-settings').onclick = openSettings;
  $('#btn-browser').onclick = openBrowserDialog;
  $('#btn-net').onclick = openNetDialog;
  $('#btn-pause-all').onclick = () => api.pauseAll();
  $('#btn-resume-all').onclick = () => api.resumeAll();
  $('#btn-clear').onclick = () => api.clearFinished();
  $('#search').addEventListener('input', (e) => { query = e.target.value.trim().toLowerCase(); renderList(); });
  document.addEventListener('keydown', (e) => {
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'n') { e.preventDefault(); openAddDialog(); }
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'f') { e.preventDefault(); $('#search').focus(); }
  });
  // Bağlantıya sürükle-bırak
  document.addEventListener('dragover', (e) => e.preventDefault());
  document.addEventListener('drop', (e) => {
    e.preventDefault();
    const text = e.dataTransfer?.getData('text/uri-list') || e.dataTransfer?.getData('text/plain') || '';
    if (isUrl(text.split('\n')[0])) openAddDialog(text.trim());
  });
  document.querySelectorAll('dialog').forEach((d) => d.addEventListener('click', (e) => { if (e.target === d) d.close(); }));

  refreshBrowserDot();
  setInterval(refreshBrowserDot, 5000);
  if (isTauri || new URLSearchParams(location.search).has('frame')) {
    $('#winctl').hidden = false;
    $('#win-min').onclick = () => api.win.minimize();
    $('#win-max').onclick = () => api.win.toggleMaximize();
    $('#win-close').onclick = () => api.win.close();
  } else document.title = 'Nehir (önizleme)';
}
main();
