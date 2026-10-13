// Tauri komutlarına ince katman. Tarayıcıda (Tauri dışında) açılırsa örnek verilerle çalışır:
// arayüzü uygulamayı derlemeden geliştirmek ve denemek için.
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';

export const isTauri = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

const real = {
  list: () => invoke('list_downloads'),
  add: (urls, video, quality) => invoke('add_downloads', { urls, video, quality: quality || null }),
  pause: (id) => invoke('pause', { id }),
  resume: (id) => invoke('resume', { id }),
  cancel: (id) => invoke('cancel', { id }),
  remove: (id, deleteFile) => invoke('remove', { id, deleteFile }),
  pauseAll: () => invoke('pause_all'),
  resumeAll: () => invoke('resume_all'),
  clearFinished: () => invoke('clear_finished'),
  getSettings: () => invoke('get_settings'),
  setSettings: (settings) => invoke('set_settings', { settings }),
  status: () => invoke('get_status'),
  openFile: (id) => invoke('open_file', { id }),
  openFolder: (id) => invoke('open_folder', { id }),
  installYtdlp: () => invoke('install_ytdlp'),
  pickFolder: () => invoke('pick_folder'),
  registerHost: () => invoke('register_native_host'),
  copyText: (text) => invoke('copy_text', { text }),
  netInspect: () => invoke('net_inspect'),
  netApply: (tcp, wifi) => invoke('net_apply', { tcp, wifi }),
  netRevert: (tcp, wifi) => invoke('net_revert', { tcp, wifi }),
  readClipboard: () => invoke('read_clipboard'),
  on: (event, cb) => listen(event, (e) => cb(e.payload)),
  win: {
    minimize: () => getCurrentWindow().minimize(),
    toggleMaximize: () => getCurrentWindow().toggleMaximize(),
    close: () => getCurrentWindow().close(),
  },
};

// ---------- Örnek veri (yalnızca tarayıcıda) ----------
function makeMock() {
  const now = Math.floor(Date.now() / 1000);
  const uid = () => crypto.randomUUID();
  const seg = (total, n, frac) => Array.from({ length: n }, (_, i) => {
    const size = Math.floor(total / n);
    const start = i * size, end = i === n - 1 ? total - 1 : start + size - 1;
    const f = Math.min(1, Math.max(0, frac + (Math.sin(i * 2.1) * 0.12)));
    return { start, end, received: Math.floor((end - start + 1) * f) };
  });
  const GB = 1024 ** 3, MB = 1024 ** 2;
  const items = [
    { file_name: 'ubuntu-24.04.2-desktop-amd64.iso', category: 'archive', state: 'downloading', total: 6.1 * GB, frac: 0.42, speed: 38 * MB, n: 8 },
    { file_name: 'Büyük Nehir Belgeseli [aB3dE].mp4', category: 'video', state: 'downloading', total: 820 * MB, frac: 0.68, speed: 12 * MB, n: 4, kind: 'video', stage: 'video' },
    { file_name: 'konferans-sunumu.pdf', category: 'document', state: 'completed', total: 18 * MB, frac: 1, speed: 0, n: 8 },
    { file_name: 'lofi-mix-2025.mp3', category: 'music', state: 'completed', total: 96 * MB, frac: 1, speed: 0, n: 8 },
    { file_name: 'obs-studio_31.0_amd64.deb', category: 'program', state: 'paused', total: 120 * MB, frac: 0.31, speed: 0, n: 8 },
    { file_name: 'veri-seti.tar.zst', category: 'archive', state: 'queued', total: -1, frac: 0, speed: 0, n: 8 },
    { file_name: 'yedek-2024.zip', category: 'archive', state: 'failed', total: 2.4 * GB, frac: 0.12, speed: 0, n: 8, error: 'Sunucu HTTP 403 döndürdü' },
    { file_name: 'yarim-kalan-arsiv.zip', category: 'archive', state: 'waiting', total: 800 * MB, frac: 0.37, speed: 0, n: 8, error: 'Ağ hatası: error sending request' },
    { file_name: 'kapak.png', category: 'image', state: 'completed', total: 3 * MB, frac: 1, speed: 0, n: 1 },
  ].concat(new URLSearchParams(location.search).has('long') ? [
    { file_name: 'https://www.netflix.com/watch/80014330?trackId=264293154&tctx=1%2C0%2C73dfcf43-28de-4d3b-8562-149233c040e0_113718699%2CeyJwYWdlSWQiOiJQU19IYjcwOGRkNi0wMTI4LTRkM2ItODU2Mi0xNDkyMzNjMDQwZTBfTDFfTjExIn0', category: 'video', state: 'failed', total: -1, frac: 0, speed: 0, n: 1, kind: 'video', error: 'ERROR: Unsupported URL: https://www.netflix.com/watch/80014330?trackId=264293154&tctx=1%2C0%2C73dfcf43-28de-4d3b-8562-149233c040e0_113718699%2CeyJwYWdlSWQiOiJQU19IYjcwOGRkNi0wMTI4LTRkM2ItODU2Mi0xNDkyMzNjMDQwZTBfTDFfTjExIn0' }
  ].map((x) => ({ ...x })) : []).map((m, i) => {
    const total = Math.floor(m.total);
    return {
      id: uid(), url: m.file_name.startsWith('http') ? m.file_name : `https://example.com/files/${encodeURIComponent(m.file_name)}`, file_name: m.file_name,
      dir: `/home/kullanici/İndirilenler/${m.category}`, kind: m.kind || 'file', category: m.category, state: m.state,
      total_bytes: total, received_bytes: total > 0 ? Math.floor(total * m.frac) : 0, speed: m.speed,
      segments: m.state === 'downloading' || m.state === 'paused' ? seg(total, m.n, m.frac) : [], segment_count: m.n, supports_ranges: true,
      error: m.error || null, stage: m.stage || null, created_at: now - i * 90, finished_at: m.state === 'completed' ? now - i * 60 : null,
    };
  });
  const listeners = new Map();
  const emit = (ev, payload) => (listeners.get(ev) || []).forEach((cb) => cb(payload));
  let settings = {
    download_dir: '/home/kullanici/İndirilenler', categorize: true, max_connections: 8, max_concurrent: 3, speed_limit_kbps: 0,
    port: 48213, capture_extensions: 'zip rar 7z iso deb pdf mp4 mkv mp3'.split(' '), language: 'auto', monitor_clipboard: true,
    notifications: true, ytdlp_path: '', video_quality: 'best', close_to_tray: true, autostart: false,
  };
  setInterval(() => {
    for (const d of items) {
      if (d.state !== 'downloading') continue;
      d.received_bytes = Math.min(d.total_bytes, d.received_bytes + d.speed * 0.5);
      d.segments = seg(d.total_bytes, d.segment_count, d.received_bytes / d.total_bytes);
      emit('download-updated', { ...d });
    }
  }, 500);
  return {
    list: async () => items.map((d) => ({ ...d })),
    add: async (urls, video, quality) => { for (const u of urls) { const d = { id: uid(), url: u, file_name: u.split('/').pop() || 'indirme', dir: settings.download_dir, kind: video ? 'video' : 'file', category: video ? 'video' : 'other', state: 'queued', total_bytes: -1, received_bytes: 0, speed: 0, segments: [], segment_count: 8, supports_ranges: false, error: null, stage: null, created_at: Math.floor(Date.now() / 1000), finished_at: null }; items.unshift(d); emit('download-updated', { ...d }); } },
    pause: async (id) => { const d = items.find((x) => x.id === id); if (d) { d.state = 'paused'; d.speed = 0; emit('download-updated', { ...d }); } },
    resume: async (id) => { const d = items.find((x) => x.id === id); if (d) { d.state = 'downloading'; d.speed = 10 * MB; emit('download-updated', { ...d }); } },
    cancel: async (id) => { const d = items.find((x) => x.id === id); if (d) { d.state = 'cancelled'; d.speed = 0; emit('download-updated', { ...d }); } },
    remove: async (id) => { const i = items.findIndex((x) => x.id === id); if (i >= 0) { items.splice(i, 1); emit('download-removed', id); } },
    pauseAll: async () => items.forEach((d) => d.state === 'downloading' && real_noop(d, 'paused', emit)),
    resumeAll: async () => items.forEach((d) => d.state === 'paused' && real_noop(d, 'downloading', emit)),
    clearFinished: async () => { for (const d of [...items]) if (['completed', 'cancelled', 'failed'].includes(d.state)) { items.splice(items.indexOf(d), 1); emit('download-removed', d.id); } },
    getSettings: async () => ({ ...settings }),
    setSettings: async (s) => { settings = { ...s }; emit('settings-changed', settings); return settings; },
    status: async () => ({ version: '1.0.0', language: new URLSearchParams(location.search).get('lang') || (settings.language === 'auto' ? ((navigator.language || 'en').toLowerCase().startsWith('tr') ? 'tr' : 'en') : settings.language), ytdlp: false, port: 48213, listening: true, extension_path: '/opt/nehir/extension/chrome', extensions_seen: { chrome: Math.floor(Date.now() / 1000) - 20 } }),
    openFile: async () => {}, openFolder: async () => {},
    installYtdlp: async () => { await new Promise((r) => setTimeout(r, 900)); },
    pickFolder: async () => '/home/kullanici/Videolar',
    registerHost: async () => ['Google Chrome', 'Brave'],
    copyText: async (text) => navigator.clipboard?.writeText(text),
    netInspect: async () => ({
      helper_installed: true, bbr_available: true, vpn_in_route: new URLSearchParams(location.search).has('vpn'), wifi_present: true,
      tcp_ok: false, tcp_applied: false, wifi_applied: false,
      items: [
        { id: 'congestion', key: 'net.ipv4.tcp_congestion_control', current: 'bbr', recommended: 'bbr', ok: true },
        { id: 'qdisc', key: 'net.core.default_qdisc', current: 'fq', recommended: 'fq', ok: true },
        { id: 'rmem_max', key: 'net.core.rmem_max', current: '4194304', recommended: '33554432', ok: false },
        { id: 'wmem_max', key: 'net.core.wmem_max', current: '4194304', recommended: '33554432', ok: false },
        { id: 'tcp_rmem', key: 'net.ipv4.tcp_rmem', current: '4096 131072 33554432', recommended: '4096 131072 33554432', ok: true },
        { id: 'tcp_wmem', key: 'net.ipv4.tcp_wmem', current: '4096 16384 4194304', recommended: '4096 65536 33554432', ok: false },
        { id: 'slow_start_idle', key: 'net.ipv4.tcp_slow_start_after_idle', current: '1', recommended: '0', ok: false },
        { id: 'mtu_probing', key: 'net.ipv4.tcp_mtu_probing', current: '0', recommended: '1', ok: false },
      ],
      ifaces: [
        { name: 'wlo1', kind: 'wifi', driver: 'iwlwifi', up: true, speed_mbps: null, mtu: 1500, default_route: true },
        { name: 'enp3s0', kind: 'ethernet', driver: 'r8169', up: false, speed_mbps: null, mtu: 1500, default_route: false },
      ],
    }),
    netApply: async () => { await new Promise((r) => setTimeout(r, 600)); },
    netRevert: async () => { await new Promise((r) => setTimeout(r, 600)); },
    readClipboard: async () => '',
    on: async (event, cb) => { listeners.set(event, [...(listeners.get(event) || []), cb]); return () => {}; },
    win: { minimize: async () => {}, toggleMaximize: async () => {}, close: async () => {} },
  };
}
function real_noop(d, state, emit) { d.state = state; if (state === 'paused') d.speed = 0; else d.speed = 10 * 1024 ** 2; emit('download-updated', { ...d }); }

export const api = isTauri ? real : makeMock();
