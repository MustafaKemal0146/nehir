'use strict';
const api = globalThis.browser ?? globalThis.chrome;
const I = globalThis.NehirI18n;
const t = (k) => I.t(k);
const $ = (id) => document.getElementById(id);
const send = (msg) => new Promise((res) => { try { const p = api.runtime.sendMessage(msg, res); if (p && p.then) p.then(res, () => res(null)); } catch { res(null); } });
const el = (tag, cls, text) => { const e = document.createElement(tag); if (cls) e.className = cls; if (text != null) e.textContent = text; return e; };

const fmtSize = (n) => {
  if (!(n > 0)) return '';
  const u = ['B', 'KB', 'MB', 'GB', 'TB']; let i = 0;
  while (n >= 1024 && i < u.length - 1) { n /= 1024; i++; }
  return (n >= 100 || i === 0 ? n.toFixed(0) : n.toFixed(1)) + ' ' + u[i];
};

let tab = null;
let online = false;

function applyLabels() {
  $('chipText').textContent = t(online ? 'statusOnline' : 'statusOffline');
  $('offlineText').textContent = t('statusOffline');
  $('launch').textContent = t('launch');
  $('launchError').textContent = t('launchFailed');
  $('pageVideoText').textContent = t('pageVideo');
  $('pageAudioText').textContent = t('pageAudio');
  $('mediaTitle').textContent = t('media');
  $('noMedia').textContent = t('noMedia');
  $('downloadsTitle').textContent = t('downloads');
  $('noDownloads').textContent = t('noDownloads');
  $('captureText').textContent = t('capture');
  $('captureHint').textContent = t('captureHint');
  $('overlayText').textContent = t('overlay');
  $('overlayHint').textContent = t('overlayHint');
}

async function refreshStatus() {
  const s = await send({ type: 'status' });
  online = !!(s && s.online);
  if (s?.lang) I.setLang(s.lang);
  $('chip').classList.toggle('on', online);
  $('offline').hidden = online || !(s && s.canLaunch);
  if (s?.settings) {
    $('intercept').checked = !!s.settings.intercept;
    $('overlay').checked = !!s.settings.overlay;
  }
  if (s?.version) $('version').textContent = 'v' + s.version;
  applyLabels();
  return online;
}

const pageOk = () => tab && /^https?:/i.test(tab.url || '');

async function sendPage(audio, btn) {
  if (!pageOk()) return;
  const r = await send({ type: 'download', url: tab.url, referer: tab.url, kind: 'video', audio, tabId: tab.id, title: tab.title });
  const span = btn.querySelector('span');
  const old = span.textContent;
  span.textContent = r && r.ok ? '✓' : '!';
  setTimeout(() => { span.textContent = old; }, 1500);
}

function mediaButton(label, m, audio) {
  const b = el('button', null, label);
  b.onclick = async () => {
    // Akış (HLS/DASH), imzalı CDN adresleri ve müzik istekleri yt-dlp ile; düz dosyalar doğrudan.
    const streaming = m.kind === 'hls' || m.kind === 'dash' || m.pageOnly || audio;
    const r = await send({ type: 'download', url: m.pageOnly ? tab.url : m.url, referer: tab.url, kind: streaming ? 'video' : 'file', audio, tabId: tab.id, title: tab.title });
    b.textContent = r && r.ok ? '✓' : '!';
    setTimeout(() => { b.textContent = label; }, 1500);
  };
  return b;
}

async function refreshMedia() {
  const list = tab ? (await send({ type: 'getMedia', tabId: tab.id })) || [] : [];
  const ul = $('media'); ul.textContent = '';
  $('noMedia').hidden = list.length > 0;
  for (const m of list) {
    const li = el('li');
    const top = el('div', 'top');
    const kind = m.kind === 'media' ? (m.type.startsWith('audio') ? 'audio' : 'video') : m.kind;
    top.append(el('span', 'badge ' + kind, kind), el('span', 'name', m.name));
    const mini = el('div', 'mini');
    mini.append(mediaButton(t('btnVideo'), m, false), mediaButton(t('btnAudio'), m, true));
    top.append(mini);
    li.append(top, el('div', 'meta', fmtSize(m.size)));
    ul.append(li);
  }
}

async function refreshDownloads() {
  const list = (await send({ type: 'downloads' })) || [];
  const ul = $('downloads'); ul.textContent = '';
  $('noDownloads').hidden = list.length > 0;
  for (const d of list.slice(0, 6)) {
    const li = el('li');
    const top = el('div', 'top');
    top.append(el('span', 'name', d.name || '…'));
    const ctl = el('div', 'ctl');
    const act = (label, action, title) => { const b = el('button', null, label); b.title = title; b.onclick = async () => { await send({ type: 'action', id: d.id, action }); refreshDownloads(); }; return b; };
    if (['downloading', 'probing', 'queued', 'processing'].includes(d.state)) ctl.append(act('⏸', 'pause', t('pause')), act('✕', 'cancel', t('cancel')));
    else if (['paused', 'failed'].includes(d.state)) ctl.append(act('▶', 'resume', t('resume')));
    top.append(ctl);
    const bar = el('div', 'bar' + (d.state === 'completed' ? ' done' : ''));
    const fill = el('div'); fill.style.width = (d.state === 'completed' ? 100 : Math.round((d.progress || 0) * 100)) + '%';
    bar.append(fill);
    const state = t('st' + d.state.charAt(0).toUpperCase() + d.state.slice(1));
    const speed = d.state === 'downloading' && d.speed > 0 ? fmtSize(d.speed) + '/s' : '';
    const meta = el('div', 'meta');
    meta.append(el('span', null, state + (d.total > 0 ? ' · ' + fmtSize(d.received) + ' / ' + fmtSize(d.total) : '')), el('span', null, speed));
    li.append(top, bar, meta);
    ul.append(li);
  }
}

$('intercept').onchange = (e) => send({ type: 'setSettings', settings: { intercept: e.target.checked } });
$('overlay').onchange = (e) => send({ type: 'setSettings', settings: { overlay: e.target.checked } });
$('launch').onclick = async () => {
  $('launchError').hidden = true;
  const r = await send({ type: 'launchApp' });
  if (!(r && r.ok)) $('launchError').hidden = false;
  refreshStatus();
};
$('pageVideo').onclick = () => sendPage(false, $('pageVideo'));
$('pageAudio').onclick = () => sendPage(true, $('pageAudio'));

(async () => {
  await I.ready;
  applyLabels();
  const forced = new URLSearchParams(location.search).get('tab'); // test kolaylığı: ?tab=<sekme kimliği>
  tab = forced ? await api.tabs.get(Number(forced)) : (await api.tabs.query({ active: true, currentWindow: true }))[0];
  $('pageVideo').disabled = $('pageAudio').disabled = !pageOk();
  await refreshStatus();
  await Promise.all([refreshMedia(), refreshDownloads()]);
  setInterval(refreshDownloads, 1000);
  setInterval(refreshMedia, 2500);
  setInterval(refreshStatus, 4000);
})();
