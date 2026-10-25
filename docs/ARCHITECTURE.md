# Mimari

Nehir üç parçadan oluşur: **çekirdek** (indirme mantığının tamamı), **masaüstü kabuğu** (pencere, tepsi) ve **tarayıcı eklentisi**.

```
┌───────────────────────── Tarayıcı ─────────────────────────┐
│ eklenti (MV3)                                              │
│  • downloads API: indirmeyi yakalar, iptal eder, devreder  │
│  • webRequest: sayfadaki video/HLS/DASH adreslerini bulur  │
│  • içerik betiği: videonun üstündeki düğme                 │
└───────────────┬──────────────────────────┬─────────────────┘
        HTTP (127.0.0.1:48213)      Native messaging (yalnızca "uygulamayı başlat")
                │                          │
┌───────────────▼──────────────┐   ┌───────▼────────┐
│ nehir-app (Tauri 2)          │   │ nehir-host     │
│  pencere · tepsi · pano      │   │ uygulamayı     │
│  ┌────────────────────────┐  │   │ arka planda    │
│  │ nehir-core             │  │   │ başlatır       │
│  │  bridge · manager      │  │   └────────────────┘
│  │  engine · video · ...  │  │
│  └────────────────────────┘  │
└──────────────────────────────┘
```

## Çalışma alanı

| Klasör | Görev |
|---|---|
| `crates/nehir-core` | Motor, yönetici, köprü, yt-dlp sarmalayıcısı, ayarlar. Arayüzden bağımsız; testlerin çoğu burada. |
| `crates/nehir-app` | Tauri kabuğu: komutlar, sistem tepsisi, pano izleme, bildirim, tek örnek, çerçevesiz pencere. |
| `crates/nehir-host` | Native messaging host. Tarayıcı, uygulama kapalıyken bununla uygulamayı başlatır. |
| `crates/nehir-daemon` | Arayüzsüz sürüm (sunucu kullanımı ve testler). |
| `ui/` | Arayüz: Vite + bağımlılıksız JavaScript. Tauri dışında örnek verilerle açılır. |
| `extension/` | MV3 tarayıcı eklentisi. `build.mjs` Chrome ve Firefox paketlerini üretir; sürümü `Cargo.toml`'dan alır. |
| `packaging/` | `.deb` üretimi ve masaüstü girdisi. |

## İndirme motoru (`engine.rs`)

1. **Yoklama:** `Range: bytes=0-0` isteğiyle boyut ve Range desteği öğrenilir.
2. **Plan:** Dosya, en çok `max_connections` parçaya (parça başına en az 512 KB) bölünür; dosya baştan ayrılır.
3. **İşçiler:** Her işçi ortak kuyruktan bir parça alır ve kendi `Range` isteğini açar. Veri 256 KB'lık tamponla `pwrite` ile yazılır.
4. **Parça bölme:** Kuyruk boşalınca boşta kalan işçi, en çok baytı kalan parçayı ikiye bölüp yarısını devralır. Bitişe doğru hız düşmez.
5. **Hata yönetimi:** 429/503 gelirse işçi sayısı azalır ve bekleme artar. Sunucu Range'i yok sayarsa (`200` dönerse) tek akışa geçilir. Ağ hataları sınırlı sayıda yeniden denenir.
6. **Devam:** Parça durumu (`start/end/received`) ilerleme bildirimleriyle dışarı verilir, yönetici bunu diske yazar.

Geçici (yeniden denenebilir) ve kalıcı hatalar ayrılır (`EngineError::is_transient`): bağlantı kopması, zaman aşımı, 408/425/429/5xx geçicidir; 403/404/410 ya da disk hatası kalıcıdır.

## Yönetici (`manager.rs`)

Durumlar: `queued → probing → downloading → (processing) → completed`, ayrıca `paused`, `waiting`, `failed`, `cancelled`.

- **Kuyruk:** Aynı anda en çok `max_concurrent` indirme çalışır.
- **Ağ bekleme:** Geçici bir hata indirmeyi `waiting`'e alır. Arka plandaki bir döngü, sunucuya TCP ile ulaşılabildiğinde (ve bilgisayar uykudan uyandığında) indirmeyi sürdürür. Ardışık bekleme sürelerinde üstel geri çekilme uygulanır; ilerleme olursa sayaç sıfırlanır.
- **Yedek adresler:** Bir video sayfası yt-dlp ile indirilemezse, eklentinin sayfada bulduğu medya adresleri sırayla denenir.
- **Kalıcılık:** Kayıtlar `downloads.json` içinde (0600) atomik olarak yazılır.
- **Adlandırma:** Dosya adı sırasıyla: açık ad → sayfa başlığı ipucu (algılanan medya) → `Content-Disposition` → URL. Çakışmada `ad (1).uzantı`.
- **Gizli çalışma klasörü:** Yarım dosyalar ve yt-dlp'nin ara dosyaları (`.f137.mp4`, `.f140.m4a`) indirme klasöründeki `.nehir-tmp/` içinde tutulur; bitince yalnızca son dosya görünür.

## Köprü (`bridge.rs`)

`127.0.0.1` üzerinde küçük bir HTTP API:

| Uç nokta | Görev |
|---|---|
| `GET /ping` | Uygulama durumu, yt-dlp var mı, yakalanacak uzantılar, uygulama dili |
| `GET /downloads` | İndirme listesi (rozet ve eklenti penceresi için) |
| `POST /download` | Yeni indirme: `url`, `filename`, `referer`, `userAgent`, `cookies`, `kind`, `quality`, `titleHint`, `fallbackUrls` |
| `POST /action` | `pause`, `resume`, `cancel`, `remove` |
| `POST /formats` | yt-dlp'den bir videonun çözünürlüklerini ve tahmini boyutlarını ister |

Erişim kuralları için [SECURITY.md](../SECURITY.md) ve README'deki *Güvenlik* bölümüne bakın.

## Video (`video.rs`)

yt-dlp süreci ayrı bir süreç grubunda çalıştırılır; çıkışı doğrudan izlenir (YouTube için başlatılan `deno` alt süreci boruları açık tutsa bile iş biter). Ara dosyalar çalışma klasörüne, bitmiş dosya indirme klasörüne yazılır (`-P temp:` / `-P home:`). Bitince ad temizlenir (`[video-id]` soneki ve tam genişlikli karakterler atılır).

## Ağ ayarları (`nettune.rs`, `packaging/nehir-nettune`)

- **İnceleme (salt okunur):** `/proc/sys/net/...` ve `/sys/class/net/...` okunur; her ayar için "şu an / önerilen / yeterli mi" hesaplanır. Arayüzler (Wi-Fi/Ethernet/VPN), sürücü adı, MTU ve varsayılan rotanın hangi arayüzden geçtiği (`/proc/net/route`) bulunur.
- **Uygulama:** Yalnızca kullanıcı onayıyla, `pkexec /usr/lib/nehir/nehir-nettune apply|revert tcp [wifi]` çağrılır. Yardımcı betik argümanı komut olarak çalıştırmaz; yalnızca `tcp` ve `wifi` kabul eder, sabit ayarları yazar ve özgün değerleri `/var/lib/nehir/nettune-backup.conf` dosyasına yedekler. polkit eylemi: `org.nehir.nettune`.
- **Test:** Yardımcı, sahte bir kök dizin ve sahte `sysctl` ile sınanır (`tests/nettune_helper.rs`); gerçek sistem ayarlarına dokunulmaz.
- **HTTP istemcisi:** varsayılan olarak HTTP/1.1. HTTP/2 tüm `Range` isteklerini tek TCP bağlantısında çoğullayacağı için çok bağlantılı indirmenin faydasını azaltır. Ancak sunucu bağlantı sayısını sınırlıyorsa (429/503) motor, işçi sayısını düşürmeden önce ikinci bir istemciyle HTTP/2'ye geçer (ALPN ile anlaşılır; sunucu desteklemiyorsa HTTP/1.1 kalır). Ayarlarda "Yalnızca HTTP/1.1" ve "HTTP/2" de seçilebilir.
- **Disk:** parça planı varsa dosya `fallocate` ile baştan ayrılır; probe sonrası `statvfs` ile boş yer denetlenir (`disk_full:<gereken>:<boş>` hatası).
- **Doğrulama** (`checksum.rs`): MD5 / SHA-1 / SHA-256 / SHA-512; beklenen özet varsa dosya tamamlanınca `Processing` + `stage=verifying` aşamasında hesaplanır. Eklenti özeti sayfadan ya da `SHA256SUMS` bağlantısından bulur.

## Platform katmanı (`platform.rs`)

İşletim sistemine bağlı her şey `nehir-core/src/platform.rs` içindedir; çağıran kod `cfg` görmez. Linux ve Windows kolları aynı imzayı taşır.

| İş | Linux | Windows |
|---|---|---|
| Konumlu yazma (`write_all_at`) | `pwrite` | `seek_write` (imleç kullanılmaz) |
| Yer ayırma ve boş alan | `fallocate`, `statvfs` | `SetEndOfFile` (`set_len`), `GetDiskFreeSpaceExW` (kota dahil) |
| Süreç ağacı (`ProcessTree`) | süreç grubu, `kill(-pid)` | **Job Object** (`KILL_ON_JOB_CLOSE`); yt-dlp → deno/ffmpeg birlikte ölür |
| Dosya/klasör açma | `xdg-open` | `explorer.exe` (`/select,` ile dosyayı seçili gösterir) |
| Uyut / kapat | `systemctl` | `SetSuspendState(false,…)` / `shutdown /s /t 0` |
| Otomatik başlatma | `~/.config/autostart/*.desktop` | `HKCU\…\Run` |
| Native messaging | tarayıcı klasörlerine manifest | manifest + her tarayıcı için kayıt defteri anahtarı |
| Mark-of-the-Web | — | `dosya:Zone.Identifier` (ZoneId=3, HostUrl, ReferrerUrl) |
| Gizli çalışma klasörü | baştaki nokta | nokta + `FILE_ATTRIBUTE_HIDDEN` |
| Dosya adı kuralları | yol ayracı, kontrol karakteri | + `< > : " \| ? *`, ayrılmış adlar, sondaki nokta/boşluk |
| Sistem dili | `LC_*` / `LANG` | `GetUserDefaultUILanguage`, `GetUserDefaultLocaleName` |
| Ağ optimizasyonu | `nettune` | yok (`NetReport.supported = false`; arayüz düğmeyi gizler) |

Windows'ta `nehir-native-host.exe`, `nehir.exe`'nin yanında durur (NSIS kurulumunda `externalBin`). Kurulum dizini: `%LOCALAPPDATA%\Nehir\`.

## Veri konumları

| Ne | Linux | Windows |
|---|---|---|
| Ayarlar | `~/.config/nehir/settings.json` | `%APPDATA%\nehir\settings.json` |
| İndirme kayıtları | `~/.local/share/nehir/downloads.json` | `%LOCALAPPDATA%\nehir\downloads.json` |
| yt-dlp, deno, ffmpeg | `~/.local/share/nehir/bin/` | `%LOCALAPPDATA%\nehir\bin\` |
| Ağ ayarı yedeği (uygulanırsa) | `/var/lib/nehir/nettune-backup.conf` | — |
| Native messaging manifestleri | tarayıcıların `NativeMessagingHosts/` klasörleri (uygulama açılışta yazar) ve `.deb` ile `/etc/...` | `%LOCALAPPDATA%\nehir\native-messaging\` + `HKCU\Software\{Google\Chrome, Microsoft\Edge, BraveSoftware\Brave-Browser, Chromium, Mozilla}\NativeMessagingHosts\org.nehir.host` |

Windows'ta veri klasörü Roaming değil **LocalAppData**'dır: yt-dlp/deno/ffmpeg yüzlerce MB tutar ve makineye özgüdür, kurumsal profil eşitlemesine girmemeli.

## Testler

`cargo test` motoru yerel bir sunucuya karşı sınar: çok parçalı indirme, Range desteği olmayan sunucu, Range'i yalan söyleyen sunucu, 503 kısıtlaması, duraklat/devam, hız sınırı; yöneticiyi (kuyruk, kalıcılık, iptal, ağ bekleme ve otomatik sürdürme, adlandırma, gizli klasör) ve köprünün erişim kurallarını doğrular. Eklenti, gerçek Chromium'da uçtan uca denenir.
