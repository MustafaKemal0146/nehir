# Değişiklik günlüğü

Biçim [Keep a Changelog](https://keepachangelog.com/tr/1.1.0/), sürümleme [SemVer](https://semver.org/lang/tr/).

## [Yayınlanmadı]

### Eklendi
- **Windows 10/11 desteği** (aynı kod tabanı, `cfg` ile). İşletim sistemine bağlı her şey `nehir-core/src/platform.rs` içinde toplandı (konumlu yazma, yer ayırma, boş alan, süreç ağacı, dosya açma, uyut/kapat, otomatik başlatma, dil).
- Windows'ta alt süreçlerin (yt-dlp → deno, ffmpeg) iptalde birlikte sonlandırılması için **Job Object**.
- Windows native messaging: manifestler ve her tarayıcı (Chrome, Edge, Brave, Chromium, Firefox) için `HKCU` kayıt defteri anahtarları; `nehir-native-host.exe` konsol penceresi açmadan ve ayrık başlatır.
- Windows'ta `yt-dlp.exe`, `deno.exe` ve `ffmpeg.exe` kurulumu (ses/görüntü birleştirme için ffmpeg gerekir).
- İndirilen dosyalara Windows'ta **Mark-of-the-Web** (`Zone.Identifier`): SmartScreen/Defender uyarısı tarayıcı indirmeleri gibi korunur.
- Windows dosya adı kuralları: yasak karakterler, ayrılmış adlar (`CON`, `NUL`, `COM1`…), sondaki nokta/boşluk.
- NSIS kurulum paketi (`packaging/windows/`), CI'de Windows işi.

### Değişti
- Windows'ta ağ optimizasyonu yoktur (`NetReport.supported`); arayüz düğmeyi gizler.
- Windows'ta veri klasörü LocalAppData'dır (Roaming değil).

### Düzeltildi
- İlk açılış sihirbazındaki logo yayın sürümünde yüklenmiyordu (`/src/logo.png` JS içinde Vite tarafından işlenmez).
- `extension/build.mjs`, Node 24'te Windows'ta yolda ASCII olmayan karakter varsa (`fs.cpSync` hatası) çöküyordu.

## [1.1.0]: 2026-10-15

### Eklendi
- **İlk açılış sihirbazı:** indirme klasörü, tarayıcı eklentisi (eklentiler sayfasını açar, klasör yolunu panoya kopyalar, eklenti bağlanınca canlı gösterir) ve `yt-dlp` kurulumu. Ayarlar'dan yeniden açılabilir.
- **İndirmeler bitince:** bilgisayarı uyut, kapat ya da Nehir'den çık. 60 saniyelik, iptal edilebilir geri sayım; bir kez uygulanır, kalıcı değildir.
- **Dosya bütünlüğü doğrulama:** MD5, SHA-1, SHA-256, SHA-512. Eklenti özeti indirmenin başladığı sayfadan (dosya adının yanındaki değer) ya da sayfadaki `SHA256SUMS` / `*.sha256` dosyasından bulur; indirme bitince otomatik doğrulanır. Ekleme penceresinden ve indirme ayrıntılarından elle de verilebilir.
- **Düğmeli bildirimler:** *Aç*, *Klasörü göster*, başarısızlıkta *Tekrar dene*; bildirime tıklamak Nehir'i öne getirir.
- **Disk ön ayırma** (`fallocate`) ve indirme başında boş yer denetimi; yer yetmezse anlaşılır hata.
- **HTTP/2 desteği:** "Otomatik" protokolde sunucu bağlantı sayısını sınırlarsa (429/503) bağlantı kapatmak yerine HTTP/2'ye geçilir. Ayarlardan "Yalnızca HTTP/1.1" ya da "HTTP/2" seçilebilir. Kullanılan protokol indirme ayrıntılarında görünür.

### Düzeltildi
- Uygulama/bilgisayar kapanırken yarıda kalan indirmeler açılışta duraklatılmış kalıyordu; artık kendiliğinden sürer (bilerek duraklatılanlar duraklatılmış kalır).
- Paket, arayüzü gömülü dosyalardan sunmak yerine `localhost:5173`'e bağlanmaya çalışabiliyordu ("Could not connect to localhost").
- Ekleme penceresinde "Video kalitesi" alanı video seçili değilken de görünüyordu.

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

[1.1.0]: https://github.com/MustafaKemal0146/nehir/releases/tag/v1.1.0
[1.0.0]: https://github.com/MustafaKemal0146/nehir/releases/tag/v1.0.0
