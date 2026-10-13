# Değişiklik günlüğü

Biçim [Keep a Changelog](https://keepachangelog.com/tr/1.1.0/), sürümleme [SemVer](https://semver.org/lang/tr/).

## [1.0.0]: 2026-10-13

İlk kararlı sürüm.

### Eklendi
- **İndirme motoru:** HTTP `Range` ile 1–32 parçalı indirme, kuyruk sonunda otomatik parça bölme, duraklat/devam, kalıcı durum, hız sınırı, 429/503 yönetimi, Range desteği olmayan sunucularda tek akışa geçiş.
- **Ağ dayanıklılığı:** Bağlantı kopunca "Ağ bekleniyor" durumu; sunucuya yeniden ulaşılınca ya da bilgisayar uykudan uyanınca otomatik devam.
- **Tarayıcı eklentisi** (Chrome, Brave, Edge, Chromium, Vivaldi, Opera, Firefox): indirme yakalama, çerez ve `Referer` aktarımı, video/HLS/DASH algılama, sağ tık menüsü, eklenti penceresi.
- **Videonun üstünde indirme düğmesi:** Video / MP3 menüsü, çözünürlük listesi (tahmini boyutlarla), gönderim bildirimi, çarpıyla geçici kapatma.
- **Video indirme:** `yt-dlp` ile 1000'den fazla site; ses+görüntü birleştirme; YouTube için `deno`; yedek medya adresleri; sayfa başlığıyla adlandırma.
- **MP3 olarak indirme.**
- **Masaüstü uygulaması** (Tauri 2): kategori kenar çubuğu, parça haritası, sistem tepsisi, pano izleme, bildirimler, çerçevesiz koyu pencere, Türkçe ve İngilizce (bilgisayar diline göre).
- **Otomatik düzen:** türe göre klasörler, yarım ve ara dosyalar için gizli çalışma klasörü, okunur dosya adları.
- **Güvenli yerel köprü** ve native messaging host (uygulama kapalıyken başlatma).
- **Ağ optimizasyonu paneli:** TCP/IP ayarlarını (BBR, `fq`, tampon üst sınırları, boşta yavaş başlangıç, MTU yoklama), ağ arayüzlerini (Wi-Fi/Ethernet sürücüsü, bağlantı durumu) ve VPN'in internet çıkışında olup olmadığını inceler. Önerileni yönetici onayıyla (polkit) uygular, yedekler ve tek tıkla geri alır. Wi-Fi güç tasarrufunu kapatma seçeneği.
- HTTP istemcisi yalnızca HTTP/1.1 kullanır (HTTP/2 tüm parçaları tek TCP bağlantısında çoğullayıp çok bağlantılı indirmeyi boşa çıkarırdı); dosya başına en çok 32 bağlantı.
- `.deb` paketi (kaldırılırken Nehir'in yazdığı ağ ayarlarını geri alır).

[1.0.0]: https://github.com/MustafaKemal0146/nehir/releases/tag/v1.0.0
