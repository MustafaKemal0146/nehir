// Nehir eklentisi — dil desteği (Türkçe / İngilizce). Arka plan, popup ve içerik betiği ortak kullanır.
// Dil seçimi: uygulama bağlıysa uygulamanın dili; değilse bilgisayarın/tarayıcının dili (Türkçe değilse İngilizce).
(function (g) {
  'use strict';
  const api = g.browser ?? g.chrome;

  const TR = {
    statusOnline: 'Bağlı', statusOffline: 'Nehir çalışmıyor',
    launch: 'Nehir’i başlat', launchFailed: 'Uygulama başlatılamadı. Nehir kurulu mu?',
    pageVideo: 'Videoyu indir', pageAudio: 'MP3 indir', pageNotSupported: 'Bu sayfa desteklenmiyor',
    media: 'Bu sayfadaki medya', noMedia: 'Henüz medya algılanmadı. Videoyu oynatınca burada görünür.',
    downloads: 'İndirmeler', noDownloads: 'Henüz indirme yok.',
    btnVideo: 'Video', btnAudio: 'MP3', pause: 'Duraklat', resume: 'Sürdür', cancel: 'İptal',
    capture: 'Tarayıcı indirmelerini yakala', captureHint: 'Dosya indirmeleri otomatik Nehir’e gider',
    overlay: 'Videoların üstünde düğme göster', overlayHint: 'Oynatıcıda “Nehir ile indir” düğmesi çıkar',
    notifSentTitle: 'Nehir’e gönderildi', notifVideoTitle: 'Video Nehir’e gönderildi', notifAudioTitle: 'Müzik Nehir’e gönderildi',
    notifOfflineTitle: 'Nehir çalışmıyor', notifOfflineBody: 'Nehir’i başlatıp tekrar deneyin. Bu sefer tarayıcı normal şekilde indirecek.',
    menuLink: 'Nehir ile indir', menuVideo: 'Sayfadaki videoyu Nehir ile indir', menuAudio: 'Sayfadaki videoyu MP3 olarak indir',
    ovTitle: 'Nehir ile indir', ovVideo: 'Video olarak indir', ovAudio: 'Müzik (MP3) olarak indir', ovHide: 'Kapat (sayfa yenilenince geri gelir)',
    ovQuality: 'Çözünürlük seç', ovBest: 'En iyi kalite', ovLoading: 'Çözünürlükler yükleniyor…', ovBack: 'Geri', ovApprox: 'Tahmini liste', ovSent: 'Nehir’e gönderildi', ovFailed: 'Gönderilemedi: Nehir çalışmıyor', ovNoYtdlp: 'yt-dlp kurulu değil (Nehir → Ayarlar)',
    stQueued: 'Sırada', stWaiting: 'Ağ bekleniyor', stProbing: 'Bağlanıyor', stDownloading: 'İndiriliyor', stProcessing: 'İşleniyor', stPaused: 'Duraklatıldı',
    stCompleted: 'Tamamlandı', stFailed: 'Başarısız', stCancelled: 'İptal edildi',
  };
  const EN = {
    statusOnline: 'Connected', statusOffline: 'Nehir is not running',
    launch: 'Start Nehir', launchFailed: 'Could not start the app. Is Nehir installed?',
    pageVideo: 'Download video', pageAudio: 'Download MP3', pageNotSupported: 'This page is not supported',
    media: 'Media on this page', noMedia: 'No media detected yet. It appears here once the video plays.',
    downloads: 'Downloads', noDownloads: 'No downloads yet.',
    btnVideo: 'Video', btnAudio: 'MP3', pause: 'Pause', resume: 'Resume', cancel: 'Cancel',
    capture: 'Capture browser downloads', captureHint: 'File downloads go to Nehir automatically',
    overlay: 'Show button over videos', overlayHint: 'A “Download with Nehir” button appears on players',
    notifSentTitle: 'Sent to Nehir', notifVideoTitle: 'Video sent to Nehir', notifAudioTitle: 'Music sent to Nehir',
    notifOfflineTitle: 'Nehir is not running', notifOfflineBody: 'Start Nehir and try again. The browser will download normally this time.',
    menuLink: 'Download with Nehir', menuVideo: 'Download this page’s video with Nehir', menuAudio: 'Download this page’s video as MP3',
    ovTitle: 'Download with Nehir', ovVideo: 'Download as video', ovAudio: 'Download as music (MP3)', ovHide: 'Close (comes back when the page reloads)',
    ovQuality: 'Choose resolution', ovBest: 'Best quality', ovLoading: 'Loading resolutions…', ovBack: 'Back', ovApprox: 'Estimated list', ovSent: 'Sent to Nehir', ovFailed: 'Could not send: Nehir is not running', ovNoYtdlp: 'yt-dlp is not installed (Nehir → Settings)',
    stQueued: 'Queued', stWaiting: 'Waiting for network', stProbing: 'Connecting', stDownloading: 'Downloading', stProcessing: 'Processing', stPaused: 'Paused',
    stCompleted: 'Completed', stFailed: 'Failed', stCancelled: 'Cancelled',
  };
  const DICT = { tr: TR, en: EN };

  function detect() {
    const list = [...(g.navigator?.languages || []), g.navigator?.language, api?.i18n?.getUILanguage?.()].filter(Boolean);
    return list.some((l) => String(l).toLowerCase().startsWith('tr')) ? 'tr' : 'en';
  }

  let lang = detect();
  const I = {
    detect,
    getLang: () => lang,
    setLang(l) { const next = l === 'tr' ? 'tr' : 'en'; const changed = next !== lang; lang = next; return changed; },
    t: (key) => DICT[lang][key] ?? EN[key] ?? key,
    /** Kayıtlı uygulama dilini okur (uygulama bağlandıkça güncellenir). */
    ready: Promise.resolve(api?.storage?.local?.get('appLanguage')).then((r) => { if (r?.appLanguage) lang = r.appLanguage === 'tr' ? 'tr' : 'en'; }).catch(() => {}),
  };
  g.NehirI18n = I;
})(globalThis);
